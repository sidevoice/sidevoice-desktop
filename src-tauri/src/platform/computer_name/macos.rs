use sidevoice_desktop_core::computer_name;

pub struct Name;

impl super::ComputerName for Name {
    fn read() -> Option<String> {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        use system_configuration_sys::dynamic_store_copy_specific::SCDynamicStoreCopyComputerName;
        // No store (a temporary session) and no encoding wanted; the name follows the Create Rule: ours to release.
        let name = unsafe { SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut()) };
        if name.is_null() {
            return None;
        }
        computer_name::given(&unsafe { CFString::wrap_under_create_rule(name) }.to_string())
    }
}
