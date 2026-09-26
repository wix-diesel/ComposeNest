//! Local ledger, Docker and OS observations for loopback TCP port planning.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    net::{Ipv4Addr, SocketAddrV4},
    time::{Duration, Instant, SystemTime},
};

#[cfg(not(windows))]
use std::net::TcpListener;

use composenest_application::host_ports::{
    self, PortCheck, PortCursor, PortInspector, PortPlan, PortReason, PortSlot,
};
use rusqlite::params;
use serde_json::Value;

use crate::{
    docker_cli::{CommandKind, DockerCli},
    sqlite::DatabaseWorker,
};

/// One scope's active reservations and Docker-published ports, observed before a scan.
pub struct PortSnapshot {
    reserved: BTreeSet<u16>,
    published: BTreeSet<u16>,
}

/// Observes external state and plans within a shared two-second request budget.
pub async fn plan_observed(
    db: &DatabaseWorker,
    docker: &DockerCli,
    scope: &str,
    slots: &[PortSlot],
    cursor: PortCursor,
) -> PortPlan {
    let started = Instant::now();
    let snapshot = match PortSnapshot::observe(db, docker, scope).await {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            return PortPlan::Rejected {
                slot: String::new(),
                conflicting_slot: None,
                reason,
                check: Some(PortCheck {
                    result: Err(reason),
                    observed_at: SystemTime::now(),
                }),
            };
        }
    };
    host_ports::plan_ports_bounded(
        slots,
        &snapshot,
        cursor,
        Duration::from_secs(2).saturating_sub(started.elapsed()),
    )
}

impl PortSnapshot {
    /// Reads all active reservations, including those owned by stopped instances, and Docker bindings.
    /// Failure of either source prevents any port from being suggested.
    pub async fn observe(
        db: &DatabaseWorker,
        docker: &DockerCli,
        scope: &str,
    ) -> Result<Self, PortReason> {
        let reserved = db.read(|connection| {
            let exists: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM management_scopes WHERE id = ?1)", params![scope],
                |row| row.get(0),
            )?;
            if !exists { return Err(rusqlite::Error::QueryReturnedNoRows.into()); }
            let mut query = connection.prepare(
                "SELECT host_port FROM port_reservations WHERE scope_id = ?1 AND host_ip = '127.0.0.1' AND protocol = 'tcp' AND status IN ('held', 'committed')"
            )?;
            let ports = query.query_map(params![scope], |row| row.get::<_, u16>(0))?
                .collect::<Result<BTreeSet<_>, _>>()?;
            Ok(ports)
        }).map_err(|_| PortReason::Unavailable)?;
        let published = tokio::time::timeout(Duration::from_secs(2), published_ports(docker))
            .await
            .map_err(|_| PortReason::Unavailable)??;
        Ok(Self {
            reserved,
            published,
        })
    }
}

impl PortInspector for PortSnapshot {
    fn inspect(&self, port: u16) -> PortCheck {
        let result = if self.reserved.contains(&port) {
            Err(PortReason::Reserved)
        } else if self.published.contains(&port) {
            Err(PortReason::Docker)
        } else {
            probe_loopback(port)
        };
        PortCheck {
            result,
            observed_at: SystemTime::now(),
        }
    }
}

async fn published_ports(docker: &DockerCli) -> Result<BTreeSet<u16>, PortReason> {
    let list = docker
        .run(
            CommandKind::Read,
            &[
                "ps".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--format".into(),
                "{{.ID}}".into(),
            ],
            Duration::from_secs(1),
        )
        .await
        .map_err(|_| PortReason::Unavailable)?;
    let ids = checked_output(&list)?;
    let mut args: Vec<OsString> = vec![
        "inspect".into(),
        "--format".into(),
        "{{json .HostConfig.PortBindings}}".into(),
    ];
    for id in ids.lines() {
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PortReason::Unavailable);
        }
        args.push(id.into());
    }
    if args.len() == 3 {
        return Ok(BTreeSet::new());
    }
    let inspected = docker
        .run(CommandKind::Read, &args, Duration::from_secs(1))
        .await
        .map_err(|_| PortReason::Unavailable)?;
    let output = checked_output(&inspected)?;
    parse_published_ports(output, args.len() - 3)
}

fn parse_published_ports(
    output: &str,
    expected_containers: usize,
) -> Result<BTreeSet<u16>, PortReason> {
    let mut ports = BTreeSet::new();
    if output.lines().count() != expected_containers {
        return Err(PortReason::Unavailable);
    }
    for line in output.lines() {
        let bindings: Value = serde_json::from_str(line).map_err(|_| PortReason::Unavailable)?;
        let object = bindings.as_object().ok_or(PortReason::Unavailable)?;
        for (container, hosts) in object {
            if !container.ends_with("/tcp") {
                continue;
            }
            if hosts.is_null() {
                continue;
            }
            let hosts = hosts.as_array().ok_or(PortReason::Unavailable)?;
            for host in hosts {
                // Treat any TCP published address conservatively: Docker and the OS
                // can disagree about dual-stack and wildcard bind interactions.
                let _address = host
                    .get("HostIp")
                    .and_then(Value::as_str)
                    .ok_or(PortReason::Unavailable)?;
                let port = host
                    .get("HostPort")
                    .and_then(Value::as_str)
                    .and_then(|text| text.parse::<u16>().ok())
                    .ok_or(PortReason::Unavailable)?;
                ports.insert(port);
            }
        }
    }
    Ok(ports)
}

fn checked_output(outcome: &crate::docker_cli::CliOutcome) -> Result<&str, PortReason> {
    if outcome.outcome_unknown
        || outcome.stdout.truncated
        || outcome.stderr.truncated
        || !outcome.status.is_some_and(|status| status.success())
    {
        return Err(PortReason::Unavailable);
    }
    std::str::from_utf8(&outcome.stdout.bytes).map_err(|_| PortReason::Unavailable)
}

#[cfg(not(windows))]
fn probe_loopback(port: u16) -> Result<(), PortReason> {
    match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(error) => Err(classify_bind_error(error)),
    }
}

#[cfg(windows)]
fn probe_loopback(port: u16) -> Result<(), PortReason> {
    use std::{mem::size_of, net::TcpListener};
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, INVALID_SOCKET, IPPROTO_TCP, SO_EXCLUSIVEADDRUSE, SOCK_STREAM, SOCKADDR,
        SOCKADDR_IN, SOCKET_ERROR, SOL_SOCKET, bind, closesocket, setsockopt, socket,
    };

    // Ensure Winsock is initialized by the standard library before using its raw API.
    let _initialization =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(classify_bind_error)?;
    // SAFETY: The socket is owned here and closed on every path; option and address
    // pointers refer to live stack values with the exact Winsock lengths.
    unsafe {
        let raw = socket(AF_INET as i32, SOCK_STREAM, IPPROTO_TCP);
        if raw == INVALID_SOCKET {
            return Err(PortReason::Unavailable);
        }
        let exclusive: i32 = 1;
        let option = setsockopt(
            raw,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            &exclusive as *const i32 as *const u8,
            size_of::<i32>() as i32,
        );
        if option == SOCKET_ERROR {
            closesocket(raw);
            return Err(PortReason::Unavailable);
        }
        let mut address: SOCKADDR_IN = std::mem::zeroed();
        address.sin_family = AF_INET;
        address.sin_port = port.to_be();
        address.sin_addr.S_un.S_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        let status = bind(
            raw,
            &address as *const _ as *const SOCKADDR,
            size_of::<SOCKADDR_IN>() as i32,
        );
        let result = if status == SOCKET_ERROR {
            Err(classify_bind_error(std::io::Error::last_os_error()))
        } else {
            Ok(())
        };
        closesocket(raw);
        result
    }
}

fn classify_bind_error(error: std::io::Error) -> PortReason {
    match error.kind() {
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => PortReason::Host,
        _ => PortReason::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn active_loopback_listener_is_not_suggested() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let snapshot = PortSnapshot {
            reserved: BTreeSet::new(),
            published: BTreeSet::new(),
        };
        assert_eq!(snapshot.inspect(port).result, Err(PortReason::Host));
        drop(listener);
        assert_eq!(snapshot.inspect(port).result, Ok(()));
    }

    #[test]
    fn active_ledger_and_docker_bindings_take_precedence() {
        let snapshot = PortSnapshot {
            reserved: BTreeSet::from([5432]),
            published: BTreeSet::from([5432, 5433]),
        };
        assert_eq!(snapshot.inspect(5432).result, Err(PortReason::Reserved));
        assert_eq!(snapshot.inspect(5433).result, Err(PortReason::Docker));
    }

    #[test]
    fn docker_wildcard_and_dual_stack_bindings_are_conservative() {
        let output = r#"{"80/tcp":[{"HostIp":"::","HostPort":"18080"}],"81/udp":[{"HostIp":"0.0.0.0","HostPort":"18081"}]}"#;
        assert_eq!(
            parse_published_ports(output, 1).unwrap(),
            BTreeSet::from([18080])
        );
        assert_eq!(
            parse_published_ports(output, 2),
            Err(PortReason::Unavailable)
        );
        assert_eq!(
            parse_published_ports("invalid", 1),
            Err(PortReason::Unavailable)
        );
    }
}
