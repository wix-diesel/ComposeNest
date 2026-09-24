use std::{io, mem::size_of, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
    },
};

use super::CliError;

pub(super) struct ProcessGroup(HANDLE);

// The handle remains open while any borrowed reference exists. Windows Job
// handles can be queried and terminated from different threads.
unsafe impl Send for ProcessGroup {}
unsafe impl Sync for ProcessGroup {}

impl ProcessGroup {
    pub(super) fn attach(pid: u32) -> Result<Self, CliError> {
        // The child may exit before assignment; an exited direct child is still
        // reaped by the caller, and assigning a live child must succeed.
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(CliError::Io(io::Error::last_os_error()));
        }
        let group = Self(job);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(CliError::Io(io::Error::last_os_error()));
        }
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(CliError::Io(io::Error::last_os_error()));
        }
        let assigned = unsafe { AssignProcessToJobObject(job, process) };
        unsafe { CloseHandle(process) };
        if assigned == 0 {
            return Err(CliError::Io(io::Error::last_os_error()));
        }
        Ok(group)
    }

    pub(super) fn terminate(&self) -> Result<(), CliError> {
        if unsafe { TerminateJobObject(self.0, 1) } == 0 {
            Err(CliError::Io(io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    pub(super) fn is_empty(&self) -> Result<bool, CliError> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut _,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                ptr::null_mut(),
            )
        };
        if queried == 0 {
            Err(CliError::Io(io::Error::last_os_error()))
        } else {
            Ok(info.ActiveProcesses == 0)
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
