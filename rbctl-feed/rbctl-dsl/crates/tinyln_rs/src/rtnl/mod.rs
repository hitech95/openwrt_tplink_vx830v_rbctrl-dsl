//! Routing netlink (rtnl) — link, address, and route management.

pub mod addr;
pub mod link;
pub mod route;

pub use addr::RtnlAddr;
pub use link::RtnlLink;
pub use route::RtnlRoute;
