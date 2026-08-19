//! Host-side VLAN interface management (transport VLAN create/delete).
//!
//! Generic rtnetlink helpers — no types from other crate modules.

use tinyln_rs::rtnl::RtnlLink;

/// Check whether a VLAN sub-interface (`parent.vlan_id`) already exists.
pub fn vlan_iface_exists(parent: &str, vlan_id: u16) -> bool {
    crate::utils::mgmt_link::iface_exists(&format!("{parent}.{vlan_id}"))
}

/// Create a host-side VLAN sub-interface (`parent.vlan_id`) and bring it up.
///
/// Tolerates `EEXIST` so the call is idempotent after a daemon restart.
/// Returns `Ok(true)` when the interface was created by this call, and
/// `Ok(false)` when it already existed.
pub fn create_vlan_iface(parent: &str, vlan_id: u16) -> Result<bool, String> {
    let name = format!("{parent}.{vlan_id}");
    let mut rtnl = RtnlLink::new().map_err(|e| format!("rtnl: {e}"))?;

    let existed = match rtnl.add_vlan(parent, vlan_id) {
        Ok(_) => false,
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => true,
        Err(e) => return Err(format!("add_vlan {name}: {e}")),
    };

    rtnl.set_up(&name).map_err(|e| format!("set_up {name}: {e}"))?;
    Ok(existed)
}

/// Delete a host-side VLAN sub-interface. Silently ignores already-gone
/// interfaces (normal during shutdown).
pub fn delete_vlan_iface(parent: &str, vlan_id: u16) {
    let name = format!("{parent}.{vlan_id}");
    if !vlan_iface_exists(parent, vlan_id) {
        return; // nothing to clean up
    }
    match RtnlLink::new().and_then(|mut r| r.del(&name)) {
        Ok(()) => log::info!("deleted {name}"),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENODEV) | Some(libc::ENXIO)) => {}
        Err(e) => log::warn!("delete {name}: {e}"),
    }
}
