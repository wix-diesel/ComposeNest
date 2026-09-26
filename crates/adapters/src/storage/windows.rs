use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Path,
};

use composenest_application::{state_store::StorageAllocation, storage::StorageError};
use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    },
};

use super::{allocation_path, digest, proof_identity, proof_json, proof_matches, random_nonce};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const DIRECTORY_MARKER: &str = ".directory.json";

pub(super) fn create(
    root_path: &Path,
    instance_id: InstanceId,
    slots: &[SlotId],
) -> Result<Vec<StorageAllocation>, StorageError> {
    if has_duplicate_slots(slots) {
        return Err(StorageError::InvalidAllocation);
    }
    let root = open_directory(root_path).map_err(|_| StorageError::Backend)?;
    let root_identity = identity(&root).map_err(|_| StorageError::Backend)?;
    let data_path = root_path.join("data");
    let ownership_path = root_path.join("ownership");
    let data = ensure_child_directory(root_path, &root, "data")?;
    let ownership = ensure_child_directory(root_path, &root, "ownership")?;
    let data_identity = identity(&data).map_err(|_| StorageError::Backend)?;
    let ownership_identity = identity(&ownership).map_err(|_| StorageError::Backend)?;
    if data_identity == root_identity
        || ownership_identity == root_identity
        || ownership_identity == data_identity
    {
        return Err(StorageError::Unverified);
    }
    let existing_identities = collect_existing_identities(
        &data_path,
        &ownership_path,
        &data,
        &ownership,
        root_identity,
        data_identity,
        ownership_identity,
    )?;

    let key = format!("{:032x}", instance_id.as_u128());
    let instance_path = data_path.join(&key);
    let owner_instance_path = ownership_path.join(&key);
    ensure_absent(&data_path, &data, &key)?;
    ensure_absent(&ownership_path, &ownership, &key)?;
    let instance = create_child_directory(&data_path, &data, &key)?;
    let owner_instance = create_child_directory(&ownership_path, &ownership, &key)?;
    let instance_identity = identity(&instance).map_err(|_| StorageError::Backend)?;
    let owner_identity = identity(&owner_instance).map_err(|_| StorageError::Backend)?;
    if [
        root_identity,
        data_identity,
        ownership_identity,
        owner_identity,
    ]
    .contains(&instance_identity)
        || [
            root_identity,
            data_identity,
            ownership_identity,
            instance_identity,
        ]
        .contains(&owner_identity)
        || existing_identities.contains(&instance_identity)
        || existing_identities.contains(&owner_identity)
    {
        return Err(StorageError::Unverified);
    }
    let marker = proof_json(
        instance_id,
        None,
        root_identity,
        data_identity,
        instance_identity,
        None,
        &random_nonce()?,
    );
    create_file(
        &owner_instance_path,
        &owner_instance,
        DIRECTORY_MARKER,
        &marker,
    )?;

    let mut identities = Vec::with_capacity(slots.len());
    let mut allocations = Vec::with_capacity(slots.len());
    for slot in slots {
        let directory = create_child_directory(&instance_path, &instance, slot.as_str())?;
        let slot_identity = identity(&directory).map_err(|_| StorageError::Backend)?;
        if [
            root_identity,
            data_identity,
            instance_identity,
            owner_identity,
        ]
        .contains(&slot_identity)
            || identities.contains(&slot_identity)
            || existing_identities.contains(&slot_identity)
        {
            return Err(StorageError::Unverified);
        }
        identities.push(slot_identity);
        let proof = proof_json(
            instance_id,
            Some(slot),
            root_identity,
            data_identity,
            instance_identity,
            Some(slot_identity),
            &random_nonce()?,
        );
        create_file(
            &owner_instance_path,
            &owner_instance,
            &format!("{}.json", slot.as_str()),
            &proof,
        )?;
        allocations.push(StorageAllocation {
            slot: slot.as_str().to_owned(),
            resource_identity: allocation_path(instance_id, slot),
            ownership_evidence: digest(&proof),
        });
    }
    Ok(allocations)
}

pub(super) fn inspect(
    root_path: &Path,
    instance_id: InstanceId,
    allocation: &StorageAllocation,
) -> StoragePresence {
    let Some(slot) = super::valid_allocation(instance_id, allocation) else {
        return StoragePresence::Unverified;
    };
    inspect_valid(root_path, instance_id, allocation, &slot)
}

fn inspect_valid(
    root_path: &Path,
    instance_id: InstanceId,
    allocation: &StorageAllocation,
    slot: &SlotId,
) -> StoragePresence {
    let Ok(root) = open_directory(root_path) else {
        return StoragePresence::Unverified;
    };
    let Ok(root_identity) = identity(&root) else {
        return StoragePresence::Unverified;
    };
    let ownership_path = root_path.join("ownership");
    let Ok(ownership) = open_child(root_path, &root, "ownership") else {
        return StoragePresence::Unverified;
    };
    let Ok(ownership_identity) = identity(&ownership) else {
        return StoragePresence::Unverified;
    };
    if ownership_identity == root_identity {
        return StoragePresence::Unverified;
    }
    let key = format!("{:032x}", instance_id.as_u128());
    let owner_instance_path = ownership_path.join(&key);
    let Ok(owner_instance) = open_child(&ownership_path, &ownership, &key) else {
        return StoragePresence::Unverified;
    };
    let Ok(owner_identity) = identity(&owner_instance) else {
        return StoragePresence::Unverified;
    };
    if [root_identity, ownership_identity].contains(&owner_identity) {
        return StoragePresence::Unverified;
    }
    let Ok(marker_bytes) = read_file_in(&owner_instance_path, &owner_instance, DIRECTORY_MARKER)
    else {
        return StoragePresence::Unverified;
    };
    let Some(marker) = proof_matches(&marker_bytes, instance_id, None, None) else {
        return StoragePresence::Unverified;
    };
    if proof_identity(&marker, "rootIdentity") != Some(root_identity) {
        return StoragePresence::Unverified;
    }
    let Ok(proof_bytes) = read_file_in(
        &owner_instance_path,
        &owner_instance,
        &format!("{}.json", slot.as_str()),
    ) else {
        return StoragePresence::Unverified;
    };
    let Some(proof) = proof_matches(
        &proof_bytes,
        instance_id,
        Some(slot),
        Some(&allocation.ownership_evidence),
    ) else {
        return StoragePresence::Unverified;
    };
    if proof_identity(&proof, "rootIdentity") != Some(root_identity)
        || proof_identity(&proof, "instanceIdentity") != proof_identity(&marker, "instanceIdentity")
    {
        return StoragePresence::Unverified;
    }

    let data_path = root_path.join("data");
    let data = match open_child(root_path, &root, "data") {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return StoragePresence::Missing,
        Err(_) => return StoragePresence::Unverified,
    };
    let Ok(data_identity) = identity(&data) else {
        return StoragePresence::Unverified;
    };
    if data_identity == root_identity || data_identity == ownership_identity {
        return StoragePresence::Unverified;
    }
    if proof_identity(&proof, "dataIdentity") != Some(data_identity)
        || proof_identity(&marker, "dataIdentity") != Some(data_identity)
    {
        return StoragePresence::Unverified;
    }
    let instance_path = data_path.join(&key);
    let instance = match open_child(&data_path, &data, &key) {
        Ok(instance) => instance,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return StoragePresence::Missing,
        Err(_) => return StoragePresence::Unverified,
    };
    let Ok(instance_identity) = identity(&instance) else {
        return StoragePresence::Unverified;
    };
    if proof_identity(&proof, "instanceIdentity") != Some(instance_identity) {
        return StoragePresence::Unverified;
    }
    if [
        root_identity,
        data_identity,
        ownership_identity,
        owner_identity,
    ]
    .contains(&instance_identity)
    {
        return StoragePresence::Unverified;
    }
    let directory = match open_child(&instance_path, &instance, slot.as_str()) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return StoragePresence::Missing,
        Err(_) => return StoragePresence::Unverified,
    };
    let Ok(slot_identity) = identity(&directory) else {
        return StoragePresence::Unverified;
    };
    if proof_identity(&proof, "slotIdentity") != Some(slot_identity)
        || slot_identity == instance_identity
        || slot_identity == data_identity
        || slot_identity == root_identity
        || slot_identity == ownership_identity
        || slot_identity == owner_identity
    {
        return StoragePresence::Unverified;
    }
    StoragePresence::Present
}

fn has_duplicate_slots(slots: &[SlotId]) -> bool {
    slots
        .iter()
        .enumerate()
        .any(|(index, slot)| slots[..index].iter().any(|earlier| earlier == slot))
}

fn collect_existing_identities(
    data_path: &Path,
    ownership_path: &Path,
    data: &File,
    ownership: &File,
    root_identity: [u64; 2],
    data_identity: [u64; 2],
    ownership_identity: [u64; 2],
) -> Result<Vec<[u64; 2]>, StorageError> {
    let mut known = vec![root_identity, data_identity, ownership_identity];
    for instance_name in directory_names(data_path)? {
        if instance_name.len() != 32
            || !instance_name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(StorageError::Unverified);
        }
        let id = u128::from_str_radix(&instance_name, 16)
            .map(InstanceId::from_u128)
            .map_err(|_| StorageError::Unverified)?;
        let instance =
            open_child(data_path, data, &instance_name).map_err(|_| StorageError::Unverified)?;
        let instance_identity = identity(&instance).map_err(|_| StorageError::Unverified)?;
        if known.contains(&instance_identity) {
            return Err(StorageError::Unverified);
        }
        known.push(instance_identity);

        let owner_instance = open_child(ownership_path, ownership, &instance_name)
            .map_err(|_| StorageError::Unverified)?;
        let owner_identity = identity(&owner_instance).map_err(|_| StorageError::Unverified)?;
        if known.contains(&owner_identity) {
            return Err(StorageError::Unverified);
        }
        known.push(owner_identity);
        let marker_bytes = read_file_in(
            &ownership_path.join(&instance_name),
            &owner_instance,
            DIRECTORY_MARKER,
        )
        .map_err(|_| StorageError::Unverified)?;
        let Some(marker) = proof_matches(&marker_bytes, id, None, None) else {
            return Err(StorageError::Unverified);
        };
        if proof_identity(&marker, "rootIdentity") != Some(root_identity)
            || proof_identity(&marker, "dataIdentity") != Some(data_identity)
            || proof_identity(&marker, "instanceIdentity") != Some(instance_identity)
        {
            return Err(StorageError::Unverified);
        }

        let instance_path = data_path.join(&instance_name);
        for slot_name in directory_names(&instance_path)? {
            let slot = SlotId::parse(&slot_name).map_err(|_| StorageError::Unverified)?;
            let slot_dir = open_child(&instance_path, &instance, &slot_name)
                .map_err(|_| StorageError::Unverified)?;
            let slot_identity = identity(&slot_dir).map_err(|_| StorageError::Unverified)?;
            if known.contains(&slot_identity) {
                return Err(StorageError::Unverified);
            }
            let proof_bytes = read_file_in(
                &ownership_path.join(&instance_name),
                &owner_instance,
                &format!("{slot_name}.json"),
            )
            .map_err(|_| StorageError::Unverified)?;
            let Some(proof) = proof_matches(&proof_bytes, id, Some(&slot), None) else {
                return Err(StorageError::Unverified);
            };
            if proof_identity(&proof, "rootIdentity") != Some(root_identity)
                || proof_identity(&proof, "dataIdentity") != Some(data_identity)
                || proof_identity(&proof, "instanceIdentity") != Some(instance_identity)
                || proof_identity(&proof, "slotIdentity") != Some(slot_identity)
            {
                return Err(StorageError::Unverified);
            }
            known.push(slot_identity);
        }
    }
    Ok(known)
}

fn directory_names(path: &Path) -> Result<Vec<String>, StorageError> {
    fs::read_dir(path)
        .map_err(|_| StorageError::Unverified)?
        .map(|entry| {
            entry
                .map_err(|_| StorageError::Unverified)?
                .file_name()
                .into_string()
                .map_err(|_| StorageError::Unverified)
        })
        .collect()
}

fn ensure_child_directory(
    parent_path: &Path,
    parent: &File,
    name: &str,
) -> Result<File, StorageError> {
    match open_child(parent_path, parent, name) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_child_directory(parent_path, parent, name)
        }
        Err(_) => Err(StorageError::Unverified),
    }
}

fn ensure_absent(parent_path: &Path, parent: &File, name: &str) -> Result<(), StorageError> {
    verify_directory_path(parent_path, parent).map_err(|_| StorageError::Unverified)?;
    match fs::symlink_metadata(parent_path.join(name)) {
        Ok(_) => Err(StorageError::Unverified),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StorageError::Unverified),
    }
}

fn create_child_directory(
    parent_path: &Path,
    parent: &File,
    name: &str,
) -> Result<File, StorageError> {
    verify_directory_path(parent_path, parent).map_err(|_| StorageError::Unverified)?;
    fs::create_dir(parent_path.join(name)).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            StorageError::Unverified
        } else {
            StorageError::Backend
        }
    })?;
    let directory = open_child(parent_path, parent, name).map_err(|_| StorageError::Unverified)?;
    if identity(parent).map_err(|_| StorageError::Unverified)?
        == identity(&directory).map_err(|_| StorageError::Unverified)?
    {
        return Err(StorageError::Unverified);
    }
    Ok(directory)
}

fn open_child(parent_path: &Path, parent: &File, name: &str) -> io::Result<File> {
    verify_directory_path(parent_path, parent)?;
    let child = open_directory(&parent_path.join(name))?;
    verify_directory_path(parent_path, parent)?;
    if identity(parent)? == identity(&child)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory aliases its parent",
        ));
    }
    Ok(child)
}

fn verify_directory_path(path: &Path, expected: &File) -> io::Result<()> {
    if identity(expected)? != identity(&open_directory(path)?)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory changed",
        ));
    }
    Ok(())
}

fn create_file(
    parent_path: &Path,
    parent: &File,
    name: &str,
    contents: &[u8],
) -> Result<(), StorageError> {
    verify_directory_path(parent_path, parent).map_err(|_| StorageError::Unverified)?;
    let path = parent_path.join(name);
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL terminated; CREATE_NEW prevents replacing any existing entry.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            StorageError::Unverified
        } else {
            StorageError::Backend
        });
    }
    // SAFETY: CreateFileW returned a fresh owned handle.
    let mut file = unsafe { File::from_raw_handle(handle) };
    file.write_all(contents)
        .map_err(|_| StorageError::Backend)?;
    file.sync_all().map_err(|_| StorageError::Backend)?;
    verify_directory_path(parent_path, parent).map_err(|_| StorageError::Unverified)
}

fn read_file_in(parent_path: &Path, parent: &File, name: &str) -> io::Result<Vec<u8>> {
    verify_directory_path(parent_path, parent)?;
    let contents = read_file(&parent_path.join(name))?;
    verify_directory_path(parent_path, parent)?;
    Ok(contents)
}

fn open_directory(path: &Path) -> io::Result<File> {
    open_checked(path, true)
}

fn open_checked(path: &Path, directory: bool) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse(&metadata) || metadata.is_dir() != directory || metadata.is_file() == directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected file type",
        ));
    }
    let file = open_handle(path, directory)?;
    let (stamp, attributes) = handle_identity(&file)?;
    let reopened = fs::symlink_metadata(path)?;
    let (current_stamp, current_attributes) = handle_identity(&open_handle(path, directory)?)?;
    if is_reparse(&reopened)
        || (attributes & 0x10 != 0) != directory
        || current_attributes != attributes
        || current_stamp != stamp
        || reopened.is_dir() != directory
        || reopened.is_file() == directory
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory changed while opening",
        ));
    }
    Ok(file)
}

fn open_handle(path: &Path, directory: bool) -> io::Result<File> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    // SAFETY: `wide` is NUL terminated and all other pointer arguments are valid for this call.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a fresh owned handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    let file = open_checked(path, false)?;
    let length = file.metadata()?.len();
    if length > super::MAX_EVIDENCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "evidence file is too large",
        ));
    }
    let mut contents = Vec::with_capacity(length as usize);
    file.take(super::MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > super::MAX_EVIDENCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "evidence file is too large",
        ));
    }
    Ok(contents)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn identity(file: &File) -> io::Result<[u64; 2]> {
    let (identity, attributes) = handle_identity(file)?;
    if attributes & 0x10 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a directory",
        ));
    }
    Ok(identity)
}

fn handle_identity(file: &File) -> io::Result<([u64; 2], u32)> {
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the handle is valid and the output pointer is writable for this API.
    let success = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful call initializes every field of the structure.
    let info = unsafe { info.assume_init() };
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reparse points are not accepted",
        ));
    }
    Ok((
        [
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        ],
        info.dwFileAttributes,
    ))
}
