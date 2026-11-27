//! CashCraft's interface.
#![forbid(unsafe_code)]

mod datafile;
mod dialogs;
mod envelopes;
pub mod gui;
mod ledger;
mod reconcile;
mod rules;
pub mod updates;
mod views;

#[cfg(test)]
mod tests;
