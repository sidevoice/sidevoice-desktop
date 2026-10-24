use sidevoice_desktop_core::computer_name;

pub struct Name;

impl super::ComputerName for Name {
    fn read() -> Option<String> {
        whoami::devicename().ok().and_then(|name| computer_name::given(&name))
    }
}
