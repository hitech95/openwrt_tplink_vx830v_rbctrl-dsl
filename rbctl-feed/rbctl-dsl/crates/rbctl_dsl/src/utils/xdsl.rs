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

/// Create a host-side VLAN sub-interface (`parent.vlan_id`) and bring it up.
///
/// Tolerates `EEXIST` so the call is idempotent after a daemon restart.
pub fn create_vlan_iface(parent: &str, vlan_id: u16) -> Result<(), String> {
    let name = format!("{parent}.{vlan_id}");
    let mut rtnl = RtnlLink::new().map_err(|e| format!("rtnl: {e}"))?;

    match rtnl.add_vlan(parent, vlan_id) {
        Ok(_) => {}
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {}
        Err(e) => return Err(format!("add_vlan {name}: {e}")),
    }

    rtnl.set_up(&name).map_err(|e| format!("set_up {name}: {e}"))
}

/// Delete a host-side VLAN sub-interface. Silently ignores already-gone
/// interfaces (normal during shutdown).
pub fn delete_vlan_iface(parent: &str, vlan_id: u16) {
    let name = format!("{parent}.{vlan_id}");
    match RtnlLink::new().and_then(|mut r| r.del(&name)) {
        Ok(()) => {}
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENODEV) | Some(libc::ENXIO)) => {}
        Err(e) => log::warn!("delete {name}: {e}"),
    }
}
