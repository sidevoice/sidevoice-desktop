use super::*;

pub struct Os;

impl WebviewData for Os {
    fn locations(home: &Path, _app_local_data: &Path) -> io::Result<Locations> {
        locations(vec![
            home.join("Library/Application Support").join(IDENTIFIER),
            home.join("Library/WebKit").join(IDENTIFIER),
            home.join("Library/Caches").join(IDENTIFIER),
        ])
    }
}
