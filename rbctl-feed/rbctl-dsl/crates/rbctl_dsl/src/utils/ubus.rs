//! ubus connection helpers — socket discovery and object registration.
//!
//! These were previously inline in `daemon/mod.rs`; moved here so they are
//! reusable by any future consumer (e.g. a standalone CLI publisher).

use crate::constants;

/// Connect to ubus and register an object. Tries the common socket paths
/// (varies between OpenWrt versions/devices). Returns `None` (with a log)
/// when ubus is unavailable — callers treat it as optional.
pub fn connect_and_register(
    obj: ubus::server::UbusObject,
) -> Option<ubus::server::UbusConnection<crate::utils::ubus_transport::UnixUbusTransport>> {
    let path = find_socket()?;
    log::info!("ubus: connecting to {path}");
    let transport = match crate::utils::ubus_transport::UnixUbusTransport::connect(&path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("ubus: connect to {path} failed: {e}");
            return None;
        }
    };
    match ubus::server::UbusConnection::connect_and_register(transport, obj) {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("ubus: HELLO/ADD_OBJECT failed: {e:?}");
            None
        }
    }
}

/// Probe the candidate ubus socket paths; first one that accepts a
/// connection wins.
pub fn find_socket() -> Option<String> {
    for path in constants::UBUS_SOCKET_PATHS {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Some((*path).into());
        }
    }
    None
}
