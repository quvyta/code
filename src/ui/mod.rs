//! The screens of QCode.

pub mod home;
pub mod logo;
pub mod page;
pub mod profiles;
pub mod providers;
pub mod settings;
pub mod setup;
pub mod switch;
pub mod workspace;
pub mod workspaces;

/// Runs `work` with `t!` speaking the language `code`, as [`qframe::i18n::active_code`] gave it on
/// the thread that asked for the work.
///
/// The runtime hands its translator only to the thread that runs `update` and `view`; on a
/// background thread `t!` shows every key as `⟦key⟧`. Work whose answer carries words for the
/// person — a store read that says why a workspace is broken, a deletion that names what is left
/// and why — takes the language along and speaks it there.
pub fn in_language<R>(code: &str, work: impl FnOnce() -> R) -> R {
    qframe::i18n::scope(std::sync::Arc::new(crate::service::translator(Some(code))), work)
}
