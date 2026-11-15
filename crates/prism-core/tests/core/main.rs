//! prism-core behaviour through its public API: patches are applied to a
//! mirrored host tree ([`prism_testing::Host`]) that checks the protocol invariants.

mod background;
mod decoration;
mod i18n;
mod motion;
mod reconcile;
mod router;
mod storage;
mod virtual_list;
