//! Windows has no WebView2 diagnostic binding implemented; the empty availability list and rejecting `apply` are the defaults.

use super::WebviewDiagnostics;

pub struct Os;

impl WebviewDiagnostics for Os {}
