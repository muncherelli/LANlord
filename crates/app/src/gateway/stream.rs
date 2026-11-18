//! the browser's live streams: one `EventSource` per topic on the web ui's `/api/events`. the
//! server pushes monitor samples and the events of an apply; this module parses them and, in
//! the browser, keeps the connection: when it drops, the page says so and reconnects with a
//! growing wait, and when the server says there is no session any more, it stops and tells.

use std::time::Duration;

use anyhow::{anyhow, Result};
use lanlord_model::apply::ApplyEvent;
use lanlord_model::monitor::Sample;

/// what a server event carries.
#[derive(Debug, PartialEq)]
pub enum Item {
    Sample(Box<Sample>),
    Apply(ApplyEvent),
}

/// what a stream tells the page.
#[derive(Debug, PartialEq)]
pub enum Update {
    Item(Item),
    /// the connection is up (again).
    Up,
    /// the connection dropped, and why; it is being retried.
    Down(String),
    /// the server has no session for this page any more; the stream stopped.
    SignedOut,
}

/// the address of the stream for `topic` (see `lanlord_model::ops::topic`).
pub fn url(topic: &str) -> String {
    format!("/api/events?topic={topic}")
}

/// the event the server sent under `name`, with its data text.
pub fn parse(name: &str, data: &str) -> Result<Item> {
    match name {
        "sample" => Ok(Item::Sample(Box::new(serde_json::from_str(data)?))),
        "apply" => Ok(Item::Apply(serde_json::from_str(data)?)),
        other => Err(anyhow!("the server sent an event called {other}")),
    }
}

/// the wait before the next try: 1 s, doubling to half a minute.
#[derive(Debug, Default)]
pub struct Backoff(u32);

impl Backoff {
    const START: Duration = Duration::from_secs(1);
    const MOST: Duration = Duration::from_secs(30);

    pub fn next_wait(&mut self) -> Duration {
        let wait = Self::START
            .saturating_mul(1u32 << self.0.min(5))
            .min(Self::MOST);
        self.0 = self.0.saturating_add(1);
        wait
    }

    /// the connection worked: start over.
    pub fn reset(&mut self) {
        self.0 = 0;
    }
}

/// what the page shows while it has no connection.
pub const DISCONNECTED: &str = "Disconnected from the server. Reconnecting...";

#[cfg(target_arch = "wasm32")]
pub use browser::Stream;

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::RefCell;
    use std::rc::{Rc, Weak};

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    use web_sys::{Event, EventSource, MessageEvent};

    use super::{parse, url, Backoff, Update, DISCONNECTED};

    type Listener = Closure<dyn FnMut(Event)>;

    struct Shared {
        topic: String,
        on: Rc<dyn Fn(Update)>,
        source: Option<EventSource>,
        // kept so the browser has something to call; replaced only by a later reconnect, never
        // from inside one of their own calls
        listeners: Vec<Listener>,
        retry: Option<Closure<dyn FnMut()>>,
        timer: Option<i32>,
        backoff: Backoff,
    }

    impl Shared {
        fn hang_up(&mut self) {
            if let Some(source) = self.source.take() {
                source.close();
            }
            if let (Some(timer), Some(window)) = (self.timer.take(), web_sys::window()) {
                window.clear_timeout_with_handle(timer);
            }
        }
    }

    impl Drop for Shared {
        fn drop(&mut self) {
            self.hang_up();
        }
    }

    /// an open stream; dropping it closes the connection and stops the retries.
    pub struct Stream {
        // held only so that dropping the stream drops the connection with it
        _connection: Rc<RefCell<Shared>>,
    }

    impl Stream {
        pub fn open(topic: &str, on: Rc<dyn Fn(Update)>) -> Self {
            let shared = Rc::new(RefCell::new(Shared {
                topic: topic.to_string(),
                on,
                source: None,
                listeners: Vec::new(),
                retry: None,
                timer: None,
                backoff: Backoff::default(),
            }));
            connect(&shared);
            Self {
                _connection: shared,
            }
        }
    }

    fn connect(shared: &Rc<RefCell<Shared>>) {
        let (address, on) = {
            let s = shared.borrow();
            (url(&s.topic), s.on.clone())
        };
        let Ok(source) = EventSource::new(&address) else {
            lost(shared);
            return;
        };
        let weak = Rc::downgrade(shared);
        let mut listeners: Vec<Listener> = Vec::new();
        let mut add = |name: &str, handler: Box<dyn FnMut(Event)>| {
            let closure = Closure::wrap(handler);
            let _ = source.add_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
            listeners.push(closure);
        };
        add("open", {
            let (weak, on) = (weak.clone(), on.clone());
            Box::new(move |_| {
                if let Some(shared) = weak.upgrade() {
                    shared.borrow_mut().backoff.reset();
                }
                on(Update::Up);
            })
        });
        for name in ["sample", "apply"] {
            let on = on.clone();
            add(
                name,
                Box::new(move |event: Event| {
                    let Some(data) = event
                        .dyn_ref::<MessageEvent>()
                        .and_then(|m| m.data().as_string())
                    else {
                        return;
                    };
                    // an event this page cannot read is skipped; the next one may be fine
                    if let Ok(item) = parse(name, &data) {
                        on(Update::Item(item));
                    }
                }),
            );
        }
        add("error", {
            let weak = weak.clone();
            Box::new(move |_| {
                if let Some(shared) = weak.upgrade() {
                    lost(&shared);
                }
            })
        });
        let mut s = shared.borrow_mut();
        s.source = Some(source);
        s.listeners = listeners;
    }

    /// the connection failed or ended: close it, say so and try again after the backoff.
    fn lost(shared: &Rc<RefCell<Shared>>) {
        let (wait, on) = {
            let mut s = shared.borrow_mut();
            s.hang_up();
            (s.backoff.next_wait(), s.on.clone())
        };
        on(Update::Down(DISCONNECTED.into()));
        let weak: Weak<RefCell<Shared>> = Rc::downgrade(shared);
        let retry = Closure::once(move || {
            let Some(shared) = weak.upgrade() else { return };
            shared.borrow_mut().timer = None;
            wasm_bindgen_futures::spawn_local(async move {
                // a stream the server refuses for want of a login would only retry for ever
                match crate::gateway::http::session_now().await {
                    Ok(None) => {
                        let on = shared.borrow().on.clone();
                        on(Update::SignedOut);
                    }
                    _ => connect(&shared),
                }
            });
        });
        let Some(window) = web_sys::window() else {
            return;
        };
        let millis = i32::try_from(wait.as_millis()).unwrap_or(30_000);
        let timer = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            retry.as_ref().unchecked_ref(),
            millis,
        );
        let mut s = shared.borrow_mut();
        s.timer = timer.ok();
        s.retry = Some(retry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_event_is_read() {
        let sample = Sample {
            at: 42,
            uptime: 7.5,
            ..Sample::default()
        };
        let text = serde_json::to_string(&sample).unwrap();
        assert_eq!(
            parse("sample", &text).unwrap(),
            Item::Sample(Box::new(sample))
        );
    }

    #[test]
    fn an_apply_event_is_read() {
        let event = ApplyEvent::Total { n: 3 };
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(parse("apply", &text).unwrap(), Item::Apply(event));
    }

    #[test]
    fn nonsense_is_an_error_not_a_panic() {
        assert!(parse("sample", "{").is_err());
        assert!(parse("apply", r#"{"event":"no-such"}"#).is_err());
        assert!(parse("surprise", "{}").is_err());
    }

    #[test]
    fn the_wait_doubles_to_half_a_minute_and_starts_over() {
        let mut backoff = Backoff::default();
        let waits: Vec<u64> = (0..8).map(|_| backoff.next_wait().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_wait().as_secs(), 1);
    }

    #[test]
    fn the_address_names_the_topic() {
        assert_eq!(url("monitor:procs"), "/api/events?topic=monitor:procs");
    }
}
