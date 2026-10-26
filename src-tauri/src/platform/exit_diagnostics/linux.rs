//! No native library whose exit destructors the probe observes runs here.

use super::ExitDiagnostics;

pub struct NativeExitDiagnostics;

impl ExitDiagnostics for NativeExitDiagnostics {}
