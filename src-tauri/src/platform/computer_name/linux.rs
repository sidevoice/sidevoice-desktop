use sidevoice_desktop_core::computer_name;

pub struct Name;

impl super::ComputerName for Name {
    fn read() -> Option<String> {
        let pretty =
            std::fs::read_to_string("/etc/machine-info").ok().and_then(|file| computer_name::pretty_hostname(&file));
        pretty
            .or_else(|| whoami::hostname().ok())
            .and_then(|name| computer_name::given(&name))
    }
}
