use std::{
    ffi::{CString, OsStr},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
};

use super::{allocation_path, digest, proof_identity, proof_json, proof_matches, random_nonce};
use composenest_application::{state_store::StorageAllocation, storage::StorageError};
use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};

const DIRECTORY_MARKER: &str = ".directory.json";

pub(super) fn create(
    root_path: &Path,
    instance_id: InstanceId,
    slots: &[SlotId],
) -> Result<Vec<StorageAllocation>, StorageError> {
    if has_duplicate_slots(slots) {
        return Err(StorageError::InvalidAllocation);
    }
    let root = open_path_directory(root_path).map_err(|_| StorageError::Backend)?;
    let root_identity = identity(&root).map_err(|_| StorageError::Backend)?;
    let data = ensure_directory_at(&root, "data")?;
    let ownership = ensure_directory_at(&root, "ownership")?;
    let data_identity = identity(&data).map_err(|_| StorageError::Backend)?;
    if [root_identity, data_identity]
        .contains(&identity(&ownership).map_err(|_| StorageError::Backend)?)
        || data_identity == root_identity
    {
        return Err(StorageError::Unverified);
    }
    let ownership_identity = identity(&ownership).map_err(|_| StorageError::Backend)?;
    let existing_identities = collect_existing_identities(
        root_path,
        &data,
        &ownership,
        root_identity,
        data_identity,
        ownership_identity,
    )?;

    let key = format!("{:032x}", instance_id.as_u128());
    ensure_absent_at(&data, &key)?;
    ensure_absent_at(&ownership, &key)?;
    let instance = create_directory_at(&data, &key)?;
    let owner_instance = create_directory_at(&ownership, &key)?;
    let instance_identity = identity(&instance).map_err(|_| StorageError::Backend)?;
    let owner_identity = identity(&owner_instance).map_err(|_| StorageError::Backend)?;
    if [
        root_identity,
        data_identity,
        ownership_identity,
        owner_identity,
    ]
    .contains(&instance_identity)
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
    create_file_at(&owner_instance, DIRECTORY_MARKER, &marker)?;

    let mut identities = Vec::with_capacity(slots.len());
    let mut allocations = Vec::with_capacity(slots.len());
    for slot in slots {
        let slot_dir = create_directory_at(&instance, slot.as_str())?;
        let slot_identity = identity(&slot_dir).map_err(|_| StorageError::Backend)?;
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
        create_file_at(&owner_instance, &format!("{}.json", slot.as_str()), &proof)?;
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
    let Ok(root) = open_path_directory(root_path) else {
        return StoragePresence::Unverified;
    };
    let Ok(root_identity) = identity(&root) else {
        return StoragePresence::Unverified;
    };
    let Ok(ownership) = open_directory_at(&root, "ownership") else {
        return StoragePresence::Unverified;
    };
    let Ok(ownership_identity) = identity(&ownership) else {
        return StoragePresence::Unverified;
    };
    if ownership_identity == root_identity {
        return StoragePresence::Unverified;
    }
    let key = format!("{:032x}", instance_id.as_u128());
    let Ok(owner_instance) = open_directory_at(&ownership, &key) else {
        return StoragePresence::Unverified;
    };
    let Ok(owner_identity) = identity(&owner_instance) else {
        return StoragePresence::Unverified;
    };
    if [root_identity, ownership_identity].contains(&owner_identity) {
        return StoragePresence::Unverified;
    }
    let Ok(marker_bytes) = read_file_at(&owner_instance, DIRECTORY_MARKER) else {
        return StoragePresence::Unverified;
    };
    let Some(marker) = proof_matches(&marker_bytes, instance_id, None, None) else {
        return StoragePresence::Unverified;
    };
    if proof_identity(&marker, "rootIdentity") != Some(root_identity) {
        return StoragePresence::Unverified;
    }
    let Ok(proof_bytes) = read_file_at(&owner_instance, &format!("{}.json", slot.as_str())) else {
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

    let data = match open_directory_at(&root, "data") {
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
    let instance = match open_directory_at(&data, &key) {
        Ok(instance) => instance,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return StoragePresence::Missing,
        Err(_) => return StoragePresence::Unverified,
    };
    let Ok(instance_identity) = identity(&instance) else {
        return StoragePresence::Unverified;
    };
    if proof_identity(&proof, "instanceIdentity") != Some(instance_identity)
        || [
            root_identity,
            data_identity,
            ownership_identity,
            owner_identity,
        ]
        .contains(&instance_identity)
    {
        return StoragePresence::Unverified;
    }
    let directory = match open_directory_at(&instance, slot.as_str()) {
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
    root_path: &Path,
    data: &File,
    ownership: &File,
    root_identity: [u64; 2],
    data_identity: [u64; 2],
    ownership_identity: [u64; 2],
) -> Result<Vec<[u64; 2]>, StorageError> {
    let data_path = root_path.join("data");
    if identity(&open_path_directory(&data_path).map_err(|_| StorageError::Unverified)?)
        .map_err(|_| StorageError::Unverified)?
        != data_identity
    {
        return Err(StorageError::Unverified);
    }

    let mut known = vec![root_identity, data_identity, ownership_identity];
    for instance_name in directory_names(&data_path)? {
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
            open_directory_at(data, &instance_name).map_err(|_| StorageError::Unverified)?;
        let instance_identity = identity(&instance).map_err(|_| StorageError::Unverified)?;
        if known.contains(&instance_identity) {
            return Err(StorageError::Unverified);
        }
        known.push(instance_identity);

        let owner_instance =
            open_directory_at(ownership, &instance_name).map_err(|_| StorageError::Unverified)?;
        let owner_identity = identity(&owner_instance).map_err(|_| StorageError::Unverified)?;
        if known.contains(&owner_identity) {
            return Err(StorageError::Unverified);
        }
        known.push(owner_identity);
        let marker = read_file_at(&owner_instance, DIRECTORY_MARKER)
            .map_err(|_| StorageError::Unverified)?;
        let Some(marker) = proof_matches(&marker, id, None, None) else {
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
            let slot_dir =
                open_directory_at(&instance, &slot_name).map_err(|_| StorageError::Unverified)?;
            let slot_identity = identity(&slot_dir).map_err(|_| StorageError::Unverified)?;
            if known.contains(&slot_identity) {
                return Err(StorageError::Unverified);
            }
            let proof_bytes = read_file_at(&owner_instance, &format!("{slot_name}.json"))
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

fn ensure_absent_at(parent: &File, name: &str) -> Result<(), StorageError> {
    match open_directory_at(parent, name) {
        Ok(_) => Err(StorageError::Unverified),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StorageError::Unverified),
    }
}

fn open_path_directory(path: &Path) -> io::Result<File> {
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    // SAFETY: `name` is NUL terminated and the returned descriptor is checked before ownership transfer.
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
        )
    };
    file_from_fd(fd)
}

fn open_directory_at(parent: &File, name: &str) -> io::Result<File> {
    open_at(parent, name, libc::O_DIRECTORY)
}

fn open_at(parent: &File, name: &str, flags: i32) -> io::Result<File> {
    let name = c_name(OsStr::new(name))?;
    // SAFETY: the parent descriptor stays open and `name` is NUL terminated.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | flags,
        )
    };
    file_from_fd(fd)
}

fn ensure_directory_at(parent: &File, name: &str) -> Result<File, StorageError> {
    match create_directory_at(parent, name) {
        Ok(directory) => Ok(directory),
        Err(StorageError::Unverified) => {
            open_directory_at(parent, name).map_err(|_| StorageError::Unverified)
        }
        Err(error) => Err(error),
    }
}

fn create_directory_at(parent: &File, name: &str) -> Result<File, StorageError> {
    let name = c_name(OsStr::new(name)).map_err(|_| StorageError::InvalidAllocation)?;
    // SAFETY: the parent descriptor stays open and `name` is NUL terminated.
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::AlreadyExists {
            return Err(StorageError::Unverified);
        }
        return Err(StorageError::Backend);
    }
    let directory = open_directory_at(
        parent,
        name.to_str().map_err(|_| StorageError::InvalidAllocation)?,
    )
    .map_err(|_| StorageError::Unverified)?;
    parent.sync_all().map_err(|_| StorageError::Backend)?;
    Ok(directory)
}

fn create_file_at(parent: &File, name: &str, contents: &[u8]) -> Result<(), StorageError> {
    let name = c_name(OsStr::new(name)).map_err(|_| StorageError::InvalidAllocation)?;
    // SAFETY: the parent descriptor stays open and `name` is NUL terminated.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            StorageError::Unverified
        } else {
            StorageError::Backend
        });
    }
    // SAFETY: openat returned a fresh descriptor whose ownership transfers to File.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(contents)
        .map_err(|_| StorageError::Backend)?;
    file.sync_all().map_err(|_| StorageError::Backend)?;
    parent.sync_all().map_err(|_| StorageError::Backend)
}

fn read_file_at(parent: &File, name: &str) -> io::Result<Vec<u8>> {
    let file = open_at(parent, name, libc::O_NONBLOCK)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > super::MAX_EVIDENCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid evidence file",
        ));
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
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

fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
}

fn file_from_fd(fd: i32) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: callers pass only fresh descriptors returned by open/openat.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn identity(file: &File) -> io::Result<[u64; 2]> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a directory",
        ));
    }
    Ok([metadata.dev(), metadata.ino()])
}
