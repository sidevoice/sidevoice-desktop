//! The frames of a panic with their addresses, and the loaded modules they can be matched to.
//!
//! What Sentry needs to name a frame after `strip = "debuginfo"` is its address and the module (debug id, load address) it is in,
//! to match against the debug files the release uploads. The modules are listed once, at start ([`loaded`]), as bare names; a
//! panic keeps only the ones that hold one of its frames ([`holding`]): the list of what else is loaded would say which
//! assistive technology or security software is installed.

use backtrace::Backtrace;
use findshlibs::SharedLibraryId;
use sentry_types::{DebugId, Uuid};

use super::record::{bare, Frame, Image};
use crate::platform::images;

/// The frames of `trace` (resolved), innermost first, from the function that panicked outwards: the capture's own frames and
/// the panic machinery's (`std::panicking`, `core::panicking`, `rust_begin_unwind`) are dropped.
pub(super) fn frames(trace: &Backtrace) -> Vec<Frame> {
    let mut frames = Vec::new();
    for frame in trace.frames() {
        let addr = frame.ip() as usize as u64;
        // libgcc's unwinder (the system's, where Linux links against it) reports one last frame whose address is 0, the end of the
        // stack; a frame without an address names nothing and Sentry cannot use it.
        if addr == 0 {
            continue;
        }
        if frame.symbols().is_empty() {
            frames.push(Frame { function: None, file: None, line: None, addr });
        }
        for symbol in frame.symbols() {
            frames.push(Frame {
                // `{:#}` leaves the hash out.
                function: symbol.name().map(|name| format!("{name:#}")),
                file: symbol.filename().and_then(|path| path.to_str()).map(str::to_owned),
                line: symbol.lineno(),
                addr,
            });
        }
    }
    trim(frames)
}

/// `frames` from the end of the panic machinery on: the run of machinery frames and what is inside it (the capture, the hook)
/// are dropped; what follows is the code that panicked. Without machinery, as it was.
pub(super) fn trim(mut frames: Vec<Frame>) -> Vec<Frame> {
    let machinery = |frame: &Frame| {
        frame.function.as_deref().is_some_and(|function| {
            function.starts_with("std::panicking::")
                || function.starts_with("core::panicking::")
                || function.starts_with("std::sys::backtrace::")
                || function.contains("rust_begin_unwind")
        })
    };
    match frames.iter().position(machinery) {
        Some(first) => {
            let run = frames[first..].iter().take_while(|frame| machinery(frame)).count();
            frames.split_off(first + run)
        }
        None => frames,
    }
}

/// Every loaded module that has a debug id, as a bare name and its place in memory. A module with no name is the program.
pub(super) fn loaded() -> Vec<Image> {
    images::loaded()
        .into_iter()
        .filter_map(|module| {
            let id = match module.debug_id? {
                SharedLibraryId::Uuid(bytes) => DebugId::from_uuid(Uuid::from_bytes(bytes)),
                SharedLibraryId::GnuBuildId(id) => debug_id_from_build_id(&id)?,
                SharedLibraryId::PdbSignature(guid, age) => DebugId::from_guid_age(&guid, age).ok()?,
                _ => return None,
            };
            let name = if module.name.is_empty() {
                std::env::current_exe().ok().and_then(|path| path.to_str().and_then(bare))?
            } else {
                bare(&module.name)?
            };
            Some(Image {
                name,
                addr: module.addr,
                size: module.size,
                vmaddr: module.vmaddr,
                id: id.to_string(),
                code_id: module.code_id.filter(|id| id.bytes().all(|b| b.is_ascii_hexdigit())),
            })
        })
        .collect()
}

/// An ELF build id as a debug id: cut or padded to 16 bytes, the first three fields flipped to big endian on a little endian
/// machine, as Breakpad and Sentry expect.
fn debug_id_from_build_id(build_id: &[u8]) -> Option<DebugId> {
    let mut data = [0u8; 16];
    let len = build_id.len().min(16);
    data[..len].copy_from_slice(&build_id[..len]);
    #[cfg(target_endian = "little")]
    {
        data[0..4].reverse();
        data[4..6].reverse();
        data[6..8].reverse();
    }
    Uuid::from_slice(&data).map(DebugId::from_uuid).ok()
}

/// The modules of `all` that hold an address of `frames`: a module no frame is in is never sent.
pub(super) fn holding(all: &[Image], frames: &[Frame]) -> Vec<Image> {
    all.iter()
        .filter(|image| frames.iter().any(|frame| frame.addr.checked_sub(image.addr).is_some_and(|at| at < image.size)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(function: &str) -> Frame {
        Frame { function: Some(function.to_owned()), file: None, line: None, addr: 1 }
    }

    #[test]
    fn the_panic_machinery_and_what_is_inside_it_are_dropped() {
        let names = |frames: Vec<Frame>| -> Vec<String> { frames.into_iter().filter_map(|f| f.function).collect() };
        let trimmed = trim(vec![
            frame("sidevoice_shared::crash::capture"),
            frame("sidevoice_shared::logging::install_panic_hook::{{closure}}"),
            frame("std::panicking::rust_panic_with_hook"),
            frame("std::panicking::begin_panic_handler::{{closure}}"),
            frame("std::sys::backtrace::__rust_end_short_backtrace"),
            frame("rust_begin_unwind"),
            frame("core::panicking::panic_fmt"),
            frame("app::run"),
            frame("main"),
        ]);
        assert_eq!(names(trimmed), ["app::run", "main"]);
        assert_eq!(names(trim(vec![frame("app::run")])), ["app::run"], "no machinery: as it was");
    }

    #[test]
    fn a_real_backtrace_has_an_address_for_every_frame_and_names_for_some() {
        // The unwinder's terminal frame (libgcc reports one with address 0 at the end of the stack) is not a frame.
        let mut here = Backtrace::new();
        here.resolve();
        let frames = frames(&here);
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|frame| frame.addr != 0), "{frames:#?}");
        assert!(frames.iter().any(|frame| frame.function.is_some()), "{frames:#?}");
    }

    #[test]
    fn only_the_modules_a_frame_is_in_are_kept() {
        let image = |name: &str, addr: u64| Image {
            name: name.to_owned(),
            addr,
            size: 0x1000,
            vmaddr: 0,
            id: "00000000-0000-0000-0000-000000000000".to_owned(),
            code_id: None,
        };
        let all = [image("app", 0x1000), image("assistive.dll", 0x9000), image("past.so", 0x2000)];
        let stack = [Frame { function: None, file: None, line: None, addr: 0x1800 }];
        let kept = holding(&all, &stack);
        assert_eq!(kept.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["app"]);
        // Just past the end of a module is not in it.
        let edge = [Frame { function: None, file: None, line: None, addr: 0x2000 }];
        assert_eq!(holding(&all, &edge).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["past.so"]);
        let outside = [Frame { function: None, file: None, line: None, addr: 0x5000 }];
        assert!(holding(&all, &outside).is_empty());
    }

    #[test]
    fn an_elf_build_id_becomes_the_debug_id_sentry_reads() {
        let id = debug_id_from_build_id(&[
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 9, 10, 11, 12, 13, 14, 15, 16, 17,
        ])
        .unwrap();
        // The first three fields are flipped on a little endian machine, the rest is as it is, cut to 16 bytes.
        let expected = if cfg!(target_endian = "little") {
            "04030201-0605-0807-090a-0b0c0d0e0f10"
        } else {
            "01020304-0506-0708-090a-0b0c0d0e0f10"
        };
        assert_eq!(id.to_string(), expected);
    }
}
