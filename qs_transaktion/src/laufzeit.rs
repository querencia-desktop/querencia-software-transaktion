//! Abstraktion über den Executor. Die Zustandsmaschine braucht nur zwei Dinge:
//! eine Aufgabe auf dem aktuellen Thread starten und eine Zeit schlafen.
//!
//! * [`TokioLaufzeit`] (Feature `tokio`): für `qs-cli` und Tests, läuft in einem
//!   `tokio::task::LocalSet`; mit `start_paused(true)` laufen Minuten in Millisekunden.
//! * [`GlibLaufzeit`] (Feature `glib`): für die GTK-Oberfläche über
//!   `glib::MainContext::default().spawn_local` und `glib::timeout_future`.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Zukunft ohne Rückgabewert, nicht `Send` (Ein-Thread-Modell).
pub type Aufgabe = Pin<Box<dyn Future<Output = ()>>>;

pub trait Laufzeit {
    /// Startet `aufgabe` auf dem aktuellen Thread; sie läuft nebenläufig weiter.
    fn starten(&self, aufgabe: Aufgabe);
    /// Liefert eine Zukunft, die nach `dauer` fertig wird.
    fn schlafen(&self, dauer: Duration) -> Aufgabe;
}

#[cfg(feature = "tokio")]
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioLaufzeit;

#[cfg(feature = "tokio")]
impl Laufzeit for TokioLaufzeit {
    fn starten(&self, aufgabe: Aufgabe) {
        // Panik außerhalb eines LocalSet ist Absicht: dann ist die Einbettung falsch.
        drop(tokio::task::spawn_local(aufgabe));
    }

    fn schlafen(&self, dauer: Duration) -> Aufgabe {
        Box::pin(tokio::time::sleep(dauer))
    }
}

#[cfg(feature = "glib")]
#[derive(Clone, Copy, Debug, Default)]
pub struct GlibLaufzeit;

#[cfg(feature = "glib")]
impl Laufzeit for GlibLaufzeit {
    fn starten(&self, aufgabe: Aufgabe) {
        // Wie SW-1: Thread-Standardkontext, sonst der globale Standardkontext.
        glib::MainContext::thread_default()
            .unwrap_or_else(glib::MainContext::default)
            .spawn_local(aufgabe);
    }

    fn schlafen(&self, dauer: Duration) -> Aufgabe {
        Box::pin(glib::timeout_future(dauer))
    }
}
