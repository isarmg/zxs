//! Maintenance internals; no CLI or Web command can allocate, publish or adopt a register.
mod ancestry;
mod genesis;
mod identity;
mod pending;
mod root_set;
mod register_flock;
mod thread_evidence;
