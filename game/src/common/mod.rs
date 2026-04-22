/*
 * common/mod.rs — cross-subsystem primitives.
 *
 * Shared utilities that multiple subsystems need but that don't belong
 * to any one of them.  Keep the surface small; anything specific to a
 * single module should live there instead.
 */

pub mod layered;

pub use layered::LayeredValue;
