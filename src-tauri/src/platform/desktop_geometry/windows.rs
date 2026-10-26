use super::DesktopGeometry;

/// Windows uses physical pixels, which the shared defaults already provide.
pub struct NativeGeometry;
impl DesktopGeometry for NativeGeometry {}
