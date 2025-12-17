//! This module defines and re-exports the interfaces for the actions repository.
//! It serves as a central point for accessing traits related to data interaction.
mod actions;
mod cursor_repository;
pub use actions::ActionsRepository;
pub use cursor_repository::CursorRepository;
