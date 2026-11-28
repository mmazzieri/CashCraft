//! CashCraft's interface.
#![forbid(unsafe_code)]

mod datafile;
mod dialogs;
mod envelopes;
mod forecast;
pub mod gui;
mod investments;
mod ledger;
mod reconcile;
mod rules;
pub mod updates;
mod views;

#[cfg(test)]
mod tests;
