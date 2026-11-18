//! prism-core behaviour through its public API: patches are applied to a
//! mirrored host tree ([`prism_testing::Host`]) that checks the protocol invariants.

mod attributes;
mod background;
mod channel;
mod close;
mod decoration;
mod fonts;
mod i18n;
mod interaction;
mod islands;
mod motion;
mod navbar;
mod pictures;
mod reconcile;
mod remote_carets;
mod request;
mod rich_edit;
mod rich_input;
mod router;
mod selection;
mod spawn;
mod storage;
mod typography;
mod virtual_list;
mod web_view;
mod widget_actions;
