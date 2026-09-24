//! Borrow host query access for one synchronous poll of a WASM execution slice.
use crate::ScanSource;
use std::cell::Cell;

thread_local! {
    static SOURCE: Cell<Option<*const dyn ScanSource>> = const { Cell::new(None) };
}

pub(super) fn enter<R>(source: Option<&dyn ScanSource>, run: impl FnOnce() -> R) -> R {
    struct Reset(Option<*const dyn ScanSource>);
    impl Drop for Reset {
        fn drop(&mut self) {
            SOURCE.set(self.0);
        }
    }

    // SAFETY: only the pointer lifetime is erased. The pointee is borrowed until
    // run returns; Reset restores the previous pointer on return or unwinding.
    // Access is thread-local and with() never exposes a reference to its caller.
    // A suspended WASM future retains no pointer; each poll installs fresh access.
    let pointer = source.map(|source| unsafe {
        std::mem::transmute::<&dyn ScanSource, *const dyn ScanSource>(source)
    });
    let _reset = Reset(SOURCE.replace(pointer));
    run()
}

pub(super) fn with<R>(call: impl FnOnce(Option<&dyn ScanSource>) -> R) -> R {
    SOURCE.with(|slot| {
        // SAFETY: enter keeps this borrow alive on this thread for the whole call.
        call(slot.get().map(|pointer| unsafe { &*pointer }))
    })
}
