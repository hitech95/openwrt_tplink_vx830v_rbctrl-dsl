//! xDSL data-plane helpers — ATM/PTM link parameter construction and
//! host-side VLAN interface management.
//!
//! These were previously inline in `daemon.rs` but are used by both the
//! daemon startup and the board worker (for deferred provisioning).

use rbctl_proto::oem::pack::{AtmEncap, AtmLinkParams, AtmLinkType, AtmQos};
use tinyln_rs::rtnl::RtnlLink;

use crate::constants;
use crate::uci_cfg::AtmConfig;

/// Build [`AtmLinkParams`] from the UCI ATM config (or defaults) plus the
/// computed transport VLAN id.
pub fn atm_params(atm: Option<&AtmConfig>, vlan_id: u16) -> AtmLinkParams<'static> {
    let a = atm.cloned().unwrap_or(AtmConfig {
        vpi: constants::ATM_DEFAULT_VPI,
        vci: constants::ATM_DEFAULT_VCI,
        encap: AtmEncap::Llc,
        link_type: AtmLinkType::Eoa,
        qos: AtmQos::Ubr,
        pcr: 0,
    });
    let mut p = AtmLinkParams::default();
    p.vpi = a.vpi;
    p.vci = a.vci;
    p.encap = a.encap;
    p.link_type = a.link_type;
    p.qos = a.qos;
    p.pcr = a.pcr;
    p.vlan_id = vlan_id;
    p
}

/// Check whether a VLAN sub-interface (`parent.vlan_id`) already exists.
pub fn vlan_iface_exists(parent: &str, vlan_id: u16) -> bool {
    crate::mgmt_link::iface_exists(&format!("{parent}.{vlan_id}"))
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
