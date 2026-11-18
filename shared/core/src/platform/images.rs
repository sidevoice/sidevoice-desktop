//! The modules loaded in this process, as the loader lists them: what a crash report's frames are matched to (the debug
//! files of the release, by debug id). Observations only; what is kept, and how a module is named, is `crash::trace`'s.
//!
//! What differs per OS is one fact (`Os::BIAS_IS_BASE`); the choice it informs is made here, once.

use findshlibs::{SharedLibrary, SharedLibraryId, TargetSharedLibrary, TARGET_SUPPORTED};

#[cfg_attr(target_os = "macos", path = "images/macos.rs")]
#[cfg_attr(target_os = "windows", path = "images/windows.rs")]
#[cfg_attr(target_os = "linux", path = "images/linux.rs")]
#[cfg_attr(target_os = "ios", path = "images/ios.rs")]
#[cfg_attr(target_os = "android", path = "images/android.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// Whether the loader's bias is the module's base address, the address Sentry wants (on Windows the segments would
    /// offset it and symbolication would be wrong). Default: it is not; the base is where the first segment is loaded, and
    /// the address it states beside it.
    const BIAS_IS_BASE: bool = false;
}

/// One loaded module as the loader says it.
pub struct Module {
    /// The name as the loader gives it: a path, or empty for the program itself. Callers keep only its last component.
    pub name: String,
    /// Where it is in memory (Sentry's `image_addr`) and where its file says it would be (`image_vmaddr`).
    pub addr: u64,
    pub vmaddr: u64,
    pub size: u64,
    /// The debug id the loader reads from the module, in the kind the OS has (a build id, a UUID, a PDB signature).
    pub debug_id: Option<SharedLibraryId>,
    /// The code id: the module's own id as text.
    pub code_id: Option<String>,
}

/// Every module loaded now. Empty where the loader cannot be asked.
pub fn loaded() -> Vec<Module> {
    let mut modules = Vec::new();
    if !TARGET_SUPPORTED {
        return modules;
    }
    TargetSharedLibrary::each(|library| {
        let (addr, vmaddr) = if Native::BIAS_IS_BASE {
            (library.virtual_memory_bias().0 as u64, 0)
        } else {
            (library.actual_load_addr().0 as u64, library.stated_load_addr().0 as u64)
        };
        modules.push(Module {
            name: library.name().to_string_lossy().into_owned(),
            addr,
            vmaddr,
            size: library.len() as u64,
            debug_id: library.debug_id(),
            code_id: library.id().map(|id| id.to_string()),
        });
    });
    modules
}
