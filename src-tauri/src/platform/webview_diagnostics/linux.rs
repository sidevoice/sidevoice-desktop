//! X11 and Wayland have no WebKitGTK diagnostic binding implemented; the empty availability list and rejecting `apply` are the defaults.

use super::WebviewDiagnostics;

pub struct Os;

impl WebviewDiagnostics for Os {}
