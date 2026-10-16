use std::cell::{Cell, RefCell};

use uuid::Uuid;

use crate::Observer;

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    static EMITTING: Cell<bool> = const { Cell::new(false) };
}

struct Context {
    observer: Observer,
    parent: Option<Uuid>,
}

/// Enables observation on this thread for the closure. Replacement scopes start new roots.
/// Threads created by the closure require their own scope. Libraries do not install panic hooks.
pub fn with_observer<T>(observer: &Observer, body: impl FnOnce() -> T) -> T {
    let Ok(previous) = CONTEXT.try_with(|context| {
        context.replace(Some(Context {
            observer: observer.clone(),
            parent: None,
        }))
    }) else {
        return body();
    };
    let _restore = Restore(previous);
    body()
}

struct Restore(Option<Context>);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = CONTEXT.try_with(|context| {
            context.replace(self.0.take());
        });
    }
}

pub(crate) fn current() -> Option<(Observer, Option<Uuid>)> {
    if EMITTING.get() {
        return None;
    }
    CONTEXT
        .try_with(|context| {
            context
                .borrow()
                .as_ref()
                .map(|c| (c.observer.clone(), c.parent))
        })
        .ok()
        .flatten()
}

pub(crate) fn enter(id: Uuid) -> Parent {
    Parent(
        CONTEXT
            .try_with(|context| {
                context
                    .borrow_mut()
                    .as_mut()
                    .and_then(|c| c.parent.replace(id))
            })
            .ok()
            .flatten(),
    )
}

pub(crate) struct Parent(Option<Uuid>);

impl Drop for Parent {
    fn drop(&mut self) {
        let _ = CONTEXT.try_with(|context| {
            if let Some(context) = context.borrow_mut().as_mut() {
                context.parent = self.0;
            }
        });
    }
}

pub(crate) fn emitting(body: impl FnOnce()) {
    if EMITTING.replace(true) {
        return;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            EMITTING.set(false);
        }
    }
    let _reset = Reset;
    body();
}
