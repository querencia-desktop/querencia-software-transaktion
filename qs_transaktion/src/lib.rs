//! # qs_transaktion – Transaktions-Zustandsmaschine für Querencia Software (SW-4)
//!
//! Typen und Traits (`Auftrag`, `Zustand`, `TransaktionsGriff`,
//! `Transaktionsmonitor`, `Paketquelle`) kommen aus `qs_typen` von SW-1.
//!
//! Dieses Crate kapselt alles, was zwischen einem Klick auf „Installieren“ in der
//! Oberfläche (SW-3) und der D-Bus-Schicht zu PackageKit (SW-1, Trait
//! [`Paketquelle`]) liegt:
//!
//! * [`Monitor`] – implementiert [`Transaktionsmonitor`]: Simulation, Bestätigung,
//!   Warteschlange (eine Transaktion zur Zeit), Sperr-Wiederholung, Wiederanbindung
//!   nach `DaemonWeg`, Zeitbegrenzung ohne Fortschritt, Abbruch in jedem Zustand.
//! * [`fehlertexte`] – deutsche Überschrift, Erklärung und empfohlene Handlung je
//!   [`Fehler`]-Variante, dazu der PackageKit-Fehlercode für das Protokoll.
//! * [`protokoll`] – Protokoll nach `~/.local/state/querencia/software.log` mit
//!   Rotation bei 1 MB.
//! * [`attrappe`] – [`AttrappeQuelle`]: schnelle, deterministische `Paketquelle`
//!   mit einstellbaren Fehlern und Verzögerungen für Tests und `qs-cli --attrappe`.
//! * [`laufzeit`] – kleine Abstraktion über den Executor (tokio `LocalSet` oder
//!   `glib::MainContext`), damit die Zustandsmaschine in Oberfläche, Tests und
//!   Kommandozeile gleich läuft.
//!
//! Alles ist Ein-Thread-Code (`Rc`, `?Send`), wie in schnittstellen.md festgelegt:
//! `melden` wird immer auf dem Thread gerufen, der `starten` gerufen hat.
//!
//! ## Zustandsübergänge
//!
//! ```text
//! starten ──► Simuliert(sim) ──┬─► WartetAufBestaetigung ──► (bestaetigen) ─┐
//!                               │   (nur Entfernen; Installieren mit Zusatz-  │
//!                               │    paketen oder > 50 MiB Download)          │
//!                               └────────────────────────────────────────────┤
//!                                                                            ▼
//!                                                   [Warteschlange: eine zur Zeit]
//!                                                                            ▼
//!            WartetAufSperre ◄── SperreBelegt ── Laeuft{prozent,status} ──► Fertig
//!                │  alle 10 s erneut, höchstens 5 min        │                (Neustart-
//!                │  (sperre_erneut_versuchen jederzeit)      │                 merkmal über
//!                └─► Fehlgeschlagen(SperreBelegt)            │                 Monitor::braucht_neustart)
//!                                                            ├─► DaemonWeg: ein Wiederanbindungsversuch,
//!                                                            │   sonst Fehlgeschlagen(DaemonWeg)
//!                                                            ├─► KeinAgent / NichtBerechtigt / sonstige
//!                                                            │   → Fehlgeschlagen(fehler)
//!                                                            └─► 10 min ohne Fortschritt → Nachfrage
//!                                                                (Laeuft mit Hinweistext), kein Abbruch
//!   abbrechen() in jedem Zustand ──► Abgebrochen
//! ```

pub mod attrappe;
pub mod fehlertexte;
pub mod laufzeit;
pub mod monitor;
pub mod protokoll;

/// Gemeinsame Typen und Traits von SW-1 (`qs_typen`), hier gebündelt
/// wiederausgeführt, damit Nutzer nur dieses Crate einbinden müssen.
pub mod typen {
    pub use qs_typen::*;
}

pub use attrappe::AttrappeQuelle;
pub use laufzeit::Laufzeit;
pub use monitor::{
    ist_ende, ist_nachfrage, Einstellungen, Monitor, NACHFRAGE_PRAEFIX, NEUSTART_HINWEIS,
};
pub use protokoll::Protokoll;
pub use typen::{
    Abbruch, Auftrag, Fehler, Fortschritt, Paket, PaketId, Paketquelle, Simulation,
    TransaktionsGriff, Transaktionsmonitor, Vorgang, Zustand,
};
