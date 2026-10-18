//! Die Zustandsmaschine: [`Monitor`] implementiert [`Transaktionsmonitor`].
//!
//! Jeder Auftrag läuft als eigene Aufgabe auf der [`Laufzeit`]. Befehle der
//! Oberfläche ([`TransaktionsGriff::abbrechen`], [`TransaktionsGriff::bestaetigen`],
//! [`TransaktionsGriff::sperre_erneut_versuchen`]) erreichen die Aufgabe über einen
//! Kanal; ein Thread-lokales Verzeichnis ordnet Griff-IDs den Kanälen zu, weil der
//! Griff laut Schnittstelle nur die `id` trägt.
//!
//! Ablauf je Auftrag siehe Crate-Dokumentation. Kurz:
//! Simulation → (Bestätigung) → Warteschlange → Ausführen mit Sperr-Wiederholung,
//! Wiederanbindung nach `DaemonWeg` und Zeitbegrenzung ohne Fortschritt → Ende.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use futures::{select, FutureExt, StreamExt};

use crate::fehlertexte;
use crate::laufzeit::Laufzeit;
use crate::protokoll::{self, Protokoll};
use crate::typen::{Fehler, Fortschritt, PaketId, Paketquelle, Simulation, Vorgang};

pub use crate::typen::{Auftrag, TransaktionsGriff, Transaktionsmonitor, Zustand};

/// Statustext, mit dem `Laeuft` meldet, dass die Transaktion einen Neustart
/// verlangt (`RequireRestart`). Danach kommt `Fertig`; zusätzlich liefert
/// [`Monitor::braucht_neustart`] das Merkmal dauerhaft.
pub const NEUSTART_HINWEIS: &str = "Ein Neustart wird nötig sein";

/// Anfang des Statustexts der Nachfrage nach Zeitüberschreitung ohne Fortschritt.
pub const NACHFRAGE_PRAEFIX: &str = "Kein Fortschritt seit ";

/// Ist dieser Zustand die Nachfrage „kein Fortschritt – weiter warten oder
/// abbrechen?“ Die Oberfläche zeigt dann zwei Knöpfe: „Weiter warten“ (nichts
/// tun) und „Abbrechen“ ([`TransaktionsGriff::abbrechen`]).
pub fn ist_nachfrage(z: &Zustand) -> bool {
    matches!(z, Zustand::Laeuft { status, .. } if status.starts_with(NACHFRAGE_PRAEFIX))
}

/// Endzustände: danach kommt keine Meldung mehr.
pub fn ist_ende(z: &Zustand) -> bool {
    matches!(
        z,
        Zustand::Fertig | Zustand::Fehlgeschlagen(_) | Zustand::Abgebrochen
    )
}

/// Zeiten und Grenzen der Zustandsmaschine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Einstellungen {
    /// Abstand zwischen zwei Sperrversuchen (Standard 10 s).
    pub sperre_wiederholung: Duration,
    /// Höchstdauer des Wartens auf die Sperre (Standard 5 min); danach
    /// `Fehlgeschlagen(SperreBelegt)`. Ein manuelles `sperre_erneut_versuchen`
    /// setzt die Uhr zurück.
    pub sperre_hoechstdauer: Duration,
    /// Zeit ohne Fortschrittsmeldung, nach der nachgefragt statt abgebrochen wird
    /// (Standard 10 min). Die Nachfrage wiederholt sich nach jeweils dieser Zeit.
    pub zeit_ohne_fortschritt: Duration,
    /// Ab dieser Downloadgröße fragt Installieren nach (Standard 50 MiB).
    pub bestaetigung_ab_download_bytes: u64,
    /// Wartezeit zwischen Erreichbarkeitsproben nach `DaemonWeg` (Standard 2 s).
    pub daemon_wartezeit: Duration,
    /// Zahl der Erreichbarkeitsproben nach `DaemonWeg` (Standard 3).
    pub daemon_versuche: u32,
    /// Höchstens so lange auf fremde laufende Transaktionen warten, bevor die
    /// Wiederanbindung trotzdem ausführt (Standard 60 s).
    pub fremde_transaktionen_hoechstdauer: Duration,
    /// Frist nach `abbrechen()` in `Laeuft`, nach der der Zustand auch ohne
    /// Rückmeldung der Quelle `Abgebrochen` wird (Standard 15 s).
    pub abbruch_frist: Duration,
}

impl Default for Einstellungen {
    fn default() -> Self {
        Self {
            sperre_wiederholung: Duration::from_secs(10),
            sperre_hoechstdauer: Duration::from_secs(5 * 60),
            zeit_ohne_fortschritt: Duration::from_secs(10 * 60),
            bestaetigung_ab_download_bytes: 50 * 1024 * 1024,
            daemon_wartezeit: Duration::from_secs(2),
            daemon_versuche: 3,
            fremde_transaktionen_hoechstdauer: Duration::from_secs(60),
            abbruch_frist: Duration::from_secs(15),
        }
    }
}

impl Einstellungen {
    /// Zahl der Sperrversuche, die in die Höchstdauer passen (mindestens 1).
    pub fn sperre_versuche(&self) -> u32 {
        let w = self.sperre_wiederholung.as_millis().max(1);
        let h = self.sperre_hoechstdauer.as_millis();
        (h.div_ceil(w)).max(1) as u32
    }
}

// ---------------------------------------------------------------------------
// Befehle der Oberfläche an die laufende Aufgabe
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Befehl {
    Abbrechen,
    Bestaetigen,
    SperreErneutVersuchen,
}

/// Griff-ID → Befehlskanal. Geteilt zwischen Monitor und den Rückrufen der Griffe.
type Verzeichnis = Rc<RefCell<HashMap<u64, mpsc::UnboundedSender<Befehl>>>>;

fn befehl_senden(verzeichnis: &Verzeichnis, id: u64, befehl: Befehl) {
    if let Some(s) = verzeichnis.borrow().get(&id) {
        let _ = s.unbounded_send(befehl);
    }
}

/// Baut einen Griff, dessen Befehle über `verzeichnis` die Aufgabe erreichen.
/// Nach dem Endzustand ist der Eintrag entfernt und alle Befehle sind wirkungslos.
fn griff_bauen(verzeichnis: &Verzeichnis, id: u64) -> TransaktionsGriff {
    let v1 = verzeichnis.clone();
    let v2 = verzeichnis.clone();
    let v3 = verzeichnis.clone();
    TransaktionsGriff::mit_rueckrufen(
        id,
        Rc::new(move |id| befehl_senden(&v1, id, Befehl::Abbrechen)),
        Rc::new(move |id| befehl_senden(&v2, id, Befehl::Bestaetigen)),
        Rc::new(move |id| befehl_senden(&v3, id, Befehl::SperreErneutVersuchen)),
    )
}

// ---------------------------------------------------------------------------
// Monitor
// ---------------------------------------------------------------------------

/// Der Transaktionsmonitor. Billig zu klonen (`Rc` innen); alle Klone teilen
/// Warteschlange und Protokoll.
#[derive(Clone)]
pub struct Monitor {
    innen: Rc<Innen>,
}

struct Innen {
    laufzeit: Rc<dyn Laufzeit>,
    einstellungen: Einstellungen,
    protokoll: Rc<Protokoll>,
    /// Griff-IDs in Startreihenfolge, die noch nicht ausgeführt wurden.
    warteschlange: RefCell<VecDeque<u64>>,
    /// Griff, der gerade ausführt (höchstens einer).
    aktiv: Cell<Option<u64>>,
    /// Aufgaben, die auf eine Änderung der Warteschlange warten.
    warter: RefCell<Vec<oneshot::Sender<()>>>,
    /// Zuletzt gemeldeter Zustand je Griff.
    zustaende: RefCell<HashMap<u64, Zustand>>,
    /// Griffe, deren Transaktion `BrauchtNeustart` gemeldet hat.
    neustart: RefCell<HashSet<u64>>,
    /// Befehlskanäle der laufenden Griffe.
    verzeichnis: Verzeichnis,
    /// Nächste Griff-ID (eindeutig je Monitor, beginnt bei 1).
    naechste_id: Cell<u64>,
}

impl Monitor {
    /// Monitor mit Standardeinstellungen und Standardprotokoll.
    pub fn neu(laufzeit: Rc<dyn Laufzeit>) -> Self {
        Self::mit(
            laufzeit,
            Einstellungen::default(),
            Rc::new(Protokoll::standard()),
        )
    }

    pub fn mit(
        laufzeit: Rc<dyn Laufzeit>,
        einstellungen: Einstellungen,
        protokoll: Rc<Protokoll>,
    ) -> Self {
        Self {
            innen: Rc::new(Innen {
                laufzeit,
                einstellungen,
                protokoll,
                warteschlange: RefCell::new(VecDeque::new()),
                aktiv: Cell::new(None),
                warter: RefCell::new(Vec::new()),
                zustaende: RefCell::new(HashMap::new()),
                neustart: RefCell::new(HashSet::new()),
                verzeichnis: Rc::new(RefCell::new(HashMap::new())),
                naechste_id: Cell::new(1),
            }),
        }
    }

    pub fn einstellungen(&self) -> &Einstellungen {
        &self.innen.einstellungen
    }

    pub fn protokoll(&self) -> &Protokoll {
        &self.innen.protokoll
    }

    /// Zuletzt gemeldeter Zustand eines Griffs (auch nach dem Ende abrufbar).
    pub fn zustand(&self, id: u64) -> Option<Zustand> {
        self.innen.zustaende.borrow().get(&id).cloned()
    }

    /// Hat die Transaktion `BrauchtNeustart` gemeldet? Merkmal zum Zustand
    /// `Fertig` (die Schnittstelle sieht dort kein Feld vor).
    pub fn braucht_neustart(&self, id: u64) -> bool {
        self.innen.neustart.borrow().contains(&id)
    }

    /// Griffe, die auf ihre Ausführung warten (Startreihenfolge).
    pub fn warteschlange(&self) -> Vec<u64> {
        self.innen.warteschlange.borrow().iter().copied().collect()
    }

    /// Griff, der gerade ausführt.
    pub fn aktiv(&self) -> Option<u64> {
        self.innen.aktiv.get()
    }
}

impl Transaktionsmonitor for Monitor {
    fn starten(
        &self,
        auftrag: Auftrag,
        quelle: Rc<dyn Paketquelle>,
        melden: Box<dyn Fn(Zustand)>,
    ) -> TransaktionsGriff {
        let id = self.innen.neue_id();
        let (sender, empfaenger) = mpsc::unbounded();
        self.innen.verzeichnis.borrow_mut().insert(id, sender);
        self.innen.warteschlange.borrow_mut().push_back(id);
        let innen = self.innen.clone();
        innen.protokoll.auftrag(
            id,
            &auftrag.vorgang,
            &auftrag
                .pakete
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>(),
        );
        let lauf = Lauf {
            innen: innen.clone(),
            id,
            quelle,
            melden: Rc::from(melden),
            befehle: RefCell::new(empfaenger),
            bestaetigt: Cell::new(false),
            letzter_schluessel: RefCell::new(None),
            letzter_protokolliert: RefCell::new(None),
            abbruch_gewuenscht: Cell::new(false),
            sperre_seit: Cell::new(None),
        };
        innen
            .laufzeit
            .starten(Box::pin(async move { lauf.ablauf(auftrag).await }));
        griff_bauen(&self.innen.verzeichnis, id)
    }

    fn wieder_anbinden(
        &self,
        quelle: Rc<dyn Paketquelle>,
        melden: Box<dyn Fn(Zustand)>,
    ) -> Vec<TransaktionsGriff> {
        let id = self.innen.neue_id();
        let (sender, empfaenger) = mpsc::unbounded();
        self.innen.verzeichnis.borrow_mut().insert(id, sender);
        let innen = self.innen.clone();
        innen.protokoll.schreiben(
            id,
            "Wiederanbindung nach Neustart: suche laufende Transaktionen",
        );
        let lauf = Lauf {
            innen: innen.clone(),
            id,
            quelle,
            melden: Rc::from(melden),
            befehle: RefCell::new(empfaenger),
            bestaetigt: Cell::new(false),
            letzter_schluessel: RefCell::new(None),
            letzter_protokolliert: RefCell::new(None),
            abbruch_gewuenscht: Cell::new(false),
            sperre_seit: Cell::new(None),
        };
        innen
            .laufzeit
            .starten(Box::pin(async move { lauf.beobachten().await }));
        vec![griff_bauen(&self.innen.verzeichnis, id)]
    }
}

impl Innen {
    fn neue_id(&self) -> u64 {
        let id = self.naechste_id.get();
        self.naechste_id.set(id + 1);
        id
    }

    fn wecken(&self) {
        for w in self.warter.borrow_mut().drain(..) {
            let _ = w.send(());
        }
    }

    /// Griff aus Warteschlange und Aktiv-Platz entfernen, Wartende wecken.
    fn freigeben(&self, id: u64) {
        self.warteschlange.borrow_mut().retain(|x| *x != id);
        if self.aktiv.get() == Some(id) {
            self.aktiv.set(None);
        }
        self.wecken();
    }
}

// ---------------------------------------------------------------------------
// Der Ablauf eines Auftrags
// ---------------------------------------------------------------------------

/// Ausgang eines Ausführungsversuchs der Quelle.
enum Ausgang {
    Fertig,
    Abgebrochen,
    Fehler(Fehler),
}

/// Ergebnis des Wartens auf die Sperre.
enum Sperrausgang {
    Weiter,
    Abgebrochen,
    Aufgegeben,
}

struct Lauf {
    innen: Rc<Innen>,
    id: u64,
    quelle: Rc<dyn Paketquelle>,
    melden: Rc<dyn Fn(Zustand)>,
    befehle: RefCell<mpsc::UnboundedReceiver<Befehl>>,
    bestaetigt: Cell<bool>,
    letzter_schluessel: RefCell<Option<String>>,
    letzter_protokolliert: RefCell<Option<Zustand>>,
    /// Abbruch wurde angefordert. Gilt für den ganzen Lauf: Meldet die Quelle danach
    /// noch einen Fehler (z. B. `SperreBelegt`), wird nicht wiederholt, sondern
    /// `Abgebrochen` gemeldet. Nur ein `Fertig` der Quelle bleibt `Fertig`.
    abbruch_gewuenscht: Cell<bool>,
    /// Beginn des aktuellen Wartens auf die Sperre (stabil über Wiederholungen).
    sperre_seit: Cell<Option<Instant>>,
}

/// Räumt beim Verlassen des Ablaufs auf, egal auf welchem Weg.
struct Aufraeumer {
    innen: Rc<Innen>,
    id: u64,
}

impl Drop for Aufraeumer {
    fn drop(&mut self) {
        self.innen.freigeben(self.id);
        self.innen.verzeichnis.borrow_mut().remove(&self.id);
    }
}

impl Lauf {
    fn melde(&self, zustand: Zustand) {
        // Protokolliert werden Zustandswechsel und Wechsel des Statustexts, nicht
        // jeder Prozentschritt (sonst wächst das Protokoll mit jeder Meldung).
        let schluessel = protokollschluessel(&zustand);
        if self.letzter_schluessel.borrow().as_deref() != Some(schluessel.as_str()) {
            let vorher = self.letzter_protokolliert.borrow().clone();
            self.innen
                .protokoll
                .zustandswechsel(self.id, vorher.as_ref(), &zustand);
            *self.letzter_schluessel.borrow_mut() = Some(schluessel);
            *self.letzter_protokolliert.borrow_mut() = Some(zustand.clone());
        }
        self.innen
            .zustaende
            .borrow_mut()
            .insert(self.id, zustand.clone());
        (self.melden)(zustand);
    }

    /// Beginn des laufenden Wartens auf die Sperre; beim ersten Aufruf jetzt.
    fn sperre_beginn(&self) -> Instant {
        match self.sperre_seit.get() {
            Some(t) => t,
            None => {
                let t = Instant::now();
                self.sperre_seit.set(Some(t));
                t
            }
        }
    }

    fn log(&self, text: &str) {
        self.innen.protokoll.schreiben(self.id, text);
    }

    fn schlafen(&self, dauer: Duration) -> futures::future::Fuse<crate::laufzeit::Aufgabe> {
        self.innen.laufzeit.schlafen(dauer).fuse()
    }

    /// Nächster Befehl der Oberfläche. Leiht den Empfänger nur innerhalb von
    /// `poll`, nie über einen `await` hinweg. Eine Bestätigung wird vermerkt,
    /// auch wenn sie in einem Zustand eintrifft, der sie nicht braucht.
    fn naechster_befehl(&self) -> impl std::future::Future<Output = Option<Befehl>> + Unpin + '_ {
        futures::future::poll_fn(move |cx| {
            let r = self.befehle.borrow_mut().poll_next_unpin(cx);
            match r {
                std::task::Poll::Ready(Some(Befehl::Bestaetigen)) => self.bestaetigt.set(true),
                std::task::Poll::Ready(Some(Befehl::Abbrechen)) => {
                    self.abbruch_gewuenscht.set(true)
                }
                _ => {}
            }
            r
        })
    }

    // -------------------------------------------------------------------
    // Hauptablauf
    // -------------------------------------------------------------------

    async fn ablauf(&self, auftrag: Auftrag) {
        let _aufraeumer = Aufraeumer {
            innen: self.innen.clone(),
            id: self.id,
        };
        let ids: Vec<PaketId> = auftrag.pakete.iter().map(|p| p.id.clone()).collect();
        let vorgang = auftrag.vorgang.clone();

        // 1. Simulation (mit Sperr-Wiederholung)
        let sim = match self.simulieren(&vorgang, &ids).await {
            Ok(sim) => sim,
            Err(ende) => {
                self.melde(ende);
                return;
            }
        };
        self.melde(Zustand::Simuliert(sim.clone()));

        // 2. Bestätigung
        if braucht_bestaetigung(&self.innen.einstellungen, &auftrag, &sim) {
            self.melde(Zustand::WartetAufBestaetigung);
            while !self.bestaetigt.get() {
                match self.naechster_befehl().await {
                    Some(Befehl::Bestaetigen) => {}
                    Some(Befehl::Abbrechen) | None => {
                        self.log("Bestätigung verweigert");
                        self.melde(Zustand::Abgebrochen);
                        return;
                    }
                    Some(Befehl::SperreErneutVersuchen) => {}
                }
            }
            self.log("Bestätigung erhalten");
        }

        // 3. Warteschlange: eine Transaktion zur Zeit, Startreihenfolge
        if !self.reihe_abwarten().await {
            self.log("Abbruch in der Warteschlange");
            self.melde(Zustand::Abgebrochen);
            return;
        }

        // 4. Ausführen
        let ende = self.ausfuehren(&vorgang, &ids).await;
        self.melde(ende);
    }

    /// Simulation mit Sperr-Wiederholung; `Err` ist der zu meldende Endzustand.
    async fn simulieren(&self, vorgang: &Vorgang, ids: &[PaketId]) -> Result<Simulation, Zustand> {
        let mut sperre = Sperre::neu(&self.innen.einstellungen);
        loop {
            let mut sim = Box::pin(self.quelle.simulieren(vorgang.clone(), ids.to_vec())).fuse();
            let ergebnis = loop {
                select! {
                    e = sim => break Some(e),
                    b = self.naechster_befehl().fuse() => match b {
                        Some(Befehl::Abbrechen) | None => break None,
                        _ => {}
                    },
                }
            };
            match ergebnis {
                None => {
                    self.log("Abbruch während der Simulation");
                    return Err(Zustand::Abgebrochen);
                }
                Some(_) if self.abbruch_gewuenscht.get() => {
                    self.log("Abbruch während der Simulation");
                    return Err(Zustand::Abgebrochen);
                }
                Some(Ok(sim)) => return Ok(sim),
                Some(Err(Fehler::SperreBelegt(wer))) => match self.sperre_warten(&mut sperre).await
                {
                    Sperrausgang::Weiter => continue,
                    Sperrausgang::Abgebrochen => return Err(Zustand::Abgebrochen),
                    Sperrausgang::Aufgegeben => {
                        return Err(Zustand::Fehlgeschlagen(Fehler::SperreBelegt(wer)))
                    }
                },
                Some(Err(f)) => {
                    self.log(&format!(
                        "Simulation fehlgeschlagen: {}",
                        fehlertexte::kurz(&f)
                    ));
                    return Err(Zustand::Fehlgeschlagen(f));
                }
            }
        }
    }

    /// Wartet, bis dieser Griff an der Reihe ist. `false` = abgebrochen.
    async fn reihe_abwarten(&self) -> bool {
        loop {
            {
                let innen = &self.innen;
                let vorn = innen.warteschlange.borrow().front().copied();
                if innen.aktiv.get().is_none() && vorn == Some(self.id) {
                    innen.warteschlange.borrow_mut().pop_front();
                    innen.aktiv.set(Some(self.id));
                    return true;
                }
            }
            let (tx, rx) = oneshot::channel::<()>();
            self.innen.warter.borrow_mut().push(tx);
            let mut rx = rx.fuse();
            select! {
                _ = rx => {}
                b = self.naechster_befehl().fuse() => match b {
                    Some(Befehl::Abbrechen) | None => return false,
                    _ => {}
                },
            }
        }
    }

    /// Ausführen mit Sperr-Wiederholung und einem Wiederanbindungsversuch.
    async fn ausfuehren(&self, vorgang: &Vorgang, ids: &[PaketId]) -> Zustand {
        let mut sperre = Sperre::neu(&self.innen.einstellungen);
        let mut wieder_angebunden = false;
        loop {
            let ausgang = self.ein_versuch(vorgang, ids).await;
            if self.abbruch_gewuenscht.get() && !matches!(ausgang, Ausgang::Fertig) {
                if let Ausgang::Fehler(f) = &ausgang {
                    self.log(&format!(
                        "Nach Abbruchwunsch meldet die Quelle {}; gilt als abgebrochen",
                        fehlertexte::kurz(f)
                    ));
                }
                return Zustand::Abgebrochen;
            }
            match ausgang {
                Ausgang::Fertig => {
                    return Zustand::Fertig;
                }
                Ausgang::Abgebrochen | Ausgang::Fehler(Fehler::Abgebrochen) => {
                    return Zustand::Abgebrochen;
                }
                Ausgang::Fehler(Fehler::SperreBelegt(wer)) => {
                    match self.sperre_warten(&mut sperre).await {
                        Sperrausgang::Weiter => continue,
                        Sperrausgang::Abgebrochen => return Zustand::Abgebrochen,
                        Sperrausgang::Aufgegeben => {
                            return Zustand::Fehlgeschlagen(Fehler::SperreBelegt(wer));
                        }
                    }
                }
                Ausgang::Fehler(Fehler::DaemonWeg) => {
                    if wieder_angebunden {
                        self.log("Verbindung erneut verloren, keine zweite Wiederanbindung");
                        return Zustand::Fehlgeschlagen(Fehler::DaemonWeg);
                    }
                    wieder_angebunden = true;
                    match self.wiederanbinden(vorgang, ids).await {
                        Ok(true) => return Zustand::Fertig,
                        Ok(false) => continue,
                        Err(z) => return z,
                    }
                }
                Ausgang::Fehler(f) => {
                    let t = fehlertexte::fehlertext(&f);
                    self.log(&format!(
                        "Fehler {}: {} – {}",
                        fehlertexte::packagekit_code(&f),
                        t.ueberschrift,
                        t.handlung
                    ));
                    return Zustand::Fehlgeschlagen(f);
                }
            }
        }
    }

    /// Ein Aufruf von `Paketquelle::ausfuehren` bis zu dessen Ende.
    async fn ein_versuch(&self, vorgang: &Vorgang, ids: &[PaketId]) -> Ausgang {
        let (tx, rx) = mpsc::unbounded::<Fortschritt>();
        let mut rx = rx;
        let abbruch = self.quelle.ausfuehren(
            vorgang.clone(),
            ids.to_vec(),
            Box::new(move |f| {
                let _ = tx.unbounded_send(f);
            }),
        );
        let mut prozent: Option<u8> = None;
        let mut status = String::from("Wird vorbereitet …");
        let mut paket_info = String::new();
        let mut abbruch_angefordert = false;
        let e = &self.innen.einstellungen;
        let mut wecker = self.schlafen(e.zeit_ohne_fortschritt);
        let mut abbruch_wecker = futures::future::Fuse::terminated();
        if self.abbruch_gewuenscht.get() {
            abbruch_angefordert = true;
            abbruch.abbrechen();
            abbruch_wecker = self.schlafen(e.abbruch_frist);
        }

        let ausgang = loop {
            select! {
                f = rx.next() => {
                    // Jede Meldung zählt als Fortschritt und setzt die Zeitbegrenzung zurück.
                    wecker = self.schlafen(e.zeit_ohne_fortschritt);
                    match f {
                        None => {
                            self.log("Quelle hat die Fortschrittsmeldung ohne Ende beendet → wie DaemonWeg");
                            break Ausgang::Fehler(Fehler::DaemonWeg);
                        }
                        Some(Fortschritt::Gestartet { .. }) => {
                            status = "Gestartet".to_string();
                            self.melde(Zustand::Laeuft { prozent, status: status.clone() });
                        }
                        Some(Fortschritt::Prozent(p)) => {
                            prozent = if p > 100 { None } else { Some(p) };
                            if prozent.is_some() {
                                self.sperre_seit.set(None);
                            }
                            self.melde(Zustand::Laeuft { prozent, status: zusammen(&status, &paket_info) });
                        }
                        Some(Fortschritt::Status(s)) => {
                            status = s;
                            paket_info.clear();
                            self.melde(Zustand::Laeuft { prozent, status: status.clone() });
                        }
                        Some(Fortschritt::PaketFortschritt { id, prozent: pp }) => {
                            let name = id.split(';').next().unwrap_or(&id).to_string();
                            paket_info = if pp > 100 { name } else { format!("{name} {pp} %") };
                            self.melde(Zustand::Laeuft { prozent, status: zusammen(&status, &paket_info) });
                        }
                        Some(Fortschritt::WartetAufSperre) => {
                            // Die Quelle wartet selbst (apt: „Warte auf Sperre“); nur anzeigen.
                            let seit = self.sperre_beginn();
                            self.melde(Zustand::WartetAufSperre { seit });
                        }
                        Some(Fortschritt::BrauchtNeustart) => {
                            self.innen.neustart.borrow_mut().insert(self.id);
                            self.log("BrauchtNeustart gemeldet");
                            self.melde(Zustand::Laeuft { prozent, status: NEUSTART_HINWEIS.to_string() });
                        }
                        Some(Fortschritt::Fertig) => break Ausgang::Fertig,
                        Some(Fortschritt::Abgebrochen) => break Ausgang::Abgebrochen,
                        Some(Fortschritt::Fehler(f)) => {
                            self.log(&format!("Quelle meldet Fehler {}", fehlertexte::kurz(&f)));
                            break Ausgang::Fehler(f);
                        }
                    }
                }
                b = self.naechster_befehl().fuse() => match b {
                    Some(Befehl::Abbrechen) if !abbruch_angefordert => {
                        abbruch_angefordert = true;
                        abbruch.abbrechen();
                        abbruch_wecker = self.schlafen(e.abbruch_frist);
                        self.log("Abbruch angefordert, warte auf Rückmeldung der Quelle");
                        self.melde(Zustand::Laeuft { prozent, status: "Wird abgebrochen …".to_string() });
                    }
                    Some(_) => {}
                    None => {}
                },
                _ = abbruch_wecker => {
                    self.log("Quelle hat den Abbruch nicht bestätigt; gilt als abgebrochen");
                    break Ausgang::Abgebrochen;
                }
                _ = wecker => {
                    let minuten = e.zeit_ohne_fortschritt.as_secs() / 60;
                    let seit = if minuten >= 1 {
                        format!("{} Minuten", minuten)
                    } else {
                        format!("{} Sekunden", e.zeit_ohne_fortschritt.as_secs())
                    };
                    self.log(&format!("Kein Fortschritt seit {}; frage nach, kein Abbruch", seit));
                    self.melde(Zustand::Laeuft {
                        prozent,
                        status: format!(
                            "{}{}. Weiter warten oder abbrechen?",
                            NACHFRAGE_PRAEFIX, seit
                        ),
                    });
                    wecker = self.schlafen(e.zeit_ohne_fortschritt);
                }
            }
        };
        drop(abbruch);
        ausgang
    }

    /// Warten nach `SperreBelegt`: alle `sperre_wiederholung` erneut, bis zur
    /// Höchstdauer; `sperre_erneut_versuchen` sofort (und setzt die Uhr zurück).
    async fn sperre_warten(&self, sperre: &mut Sperre) -> Sperrausgang {
        if self.abbruch_gewuenscht.get() {
            return Sperrausgang::Abgebrochen;
        }
        if sperre.versuche > 0 {
            self.log(&format!(
                "Sperre weiterhin belegt (Versuch {})",
                sperre.versuche
            ));
        }
        let seit = self.sperre_beginn();
        self.melde(Zustand::WartetAufSperre { seit });
        if sperre.versuche >= sperre.hoechstens {
            self.log(&format!(
                "Sperre nach {} Versuchen weiterhin belegt → Fehlgeschlagen",
                sperre.versuche
            ));
            return Sperrausgang::Aufgegeben;
        }
        let mut wecker = self.schlafen(self.innen.einstellungen.sperre_wiederholung);
        loop {
            select! {
                _ = wecker => {
                    sperre.versuche += 1;
                    return Sperrausgang::Weiter;
                }
                b = self.naechster_befehl().fuse() => match b {
                    Some(Befehl::Abbrechen) | None => {
                        self.log("Abbruch während des Wartens auf die Sperre");
                        return Sperrausgang::Abgebrochen;
                    }
                    Some(Befehl::SperreErneutVersuchen) => {
                        self.log("Sperre auf Wunsch sofort erneut versuchen");
                        sperre.versuche = 1;
                        return Sperrausgang::Weiter;
                    }
                    Some(Befehl::Bestaetigen) => {}
                },
            }
        }
    }

    /// Ein Wiederanbindungsversuch nach `DaemonWeg`.
    ///
    /// `Ok(true)`: nichts mehr zu tun (die Pakete sind schon im Zielzustand) → `Fertig`. `Ok(false)`: erneut ausführen.
    /// `Err(zustand)`: Endzustand (Daemon bleibt weg, Abbruch, anderer Fehler).
    async fn wiederanbinden(&self, vorgang: &Vorgang, ids: &[PaketId]) -> Result<bool, Zustand> {
        let e = &self.innen.einstellungen;
        self.log("Verbindung zur Paketverwaltung verloren; ein Wiederanbindungsversuch");
        self.melde(Zustand::Laeuft {
            prozent: None,
            status: "Verbindung zur Paketverwaltung verloren, versuche Wiederanbindung …"
                .to_string(),
        });

        // a) Ist der Daemon wieder erreichbar?
        let mut laufende: Option<Vec<String>> = None;
        for versuch in 1..=e.daemon_versuche.max(1) {
            match self.quelle.laufende_transaktionen().await {
                Ok(l) => {
                    laufende = Some(l);
                    break;
                }
                Err(f) => {
                    self.log(&format!(
                        "Daemon nicht erreichbar (Probe {}/{}): {}",
                        versuch,
                        e.daemon_versuche.max(1),
                        fehlertexte::kurz(&f)
                    ));
                    if versuch < e.daemon_versuche.max(1)
                        && !self.schlafen_oder_abbruch(e.daemon_wartezeit).await
                    {
                        return Err(Zustand::Abgebrochen);
                    }
                }
            }
        }
        let Some(mut laufende) = laufende else {
            self.log("Daemon bleibt unerreichbar → Fehlgeschlagen(DaemonWeg)");
            return Err(Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
        };

        // b) Laufende (fremde oder eigene frühere) Transaktionen abwarten, begrenzt.
        if !laufende.is_empty() {
            self.log(&format!(
                "{} laufende Transaktion(en) beim Daemon, warte ab: {}",
                laufende.len(),
                protokoll::kuerze_pakete(&laufende)
            ));
            self.melde(Zustand::Laeuft {
                prozent: None,
                status: "Warte auf eine laufende Transaktion der Paketverwaltung …".to_string(),
            });
            let mut gewartet = Duration::ZERO;
            while !laufende.is_empty() && gewartet < e.fremde_transaktionen_hoechstdauer {
                if !self.schlafen_oder_abbruch(e.daemon_wartezeit).await {
                    return Err(Zustand::Abgebrochen);
                }
                gewartet += e.daemon_wartezeit;
                match self.quelle.laufende_transaktionen().await {
                    Ok(l) => laufende = l,
                    Err(f) => {
                        self.log(&format!("Daemon erneut weg: {}", fehlertexte::kurz(&f)));
                        return Err(Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
                    }
                }
            }
        }

        // c) Ist der Zielzustand schon erreicht? (Der Daemon kann nach dpkg, aber
        //    vor `Finished` gestorben sein.) Sonst dieselbe Paketliste erneut ausführen.
        match self.ziel_erreicht(vorgang, ids).await {
            Ok(true) => {
                self.log("Wiederanbindung: Zielzustand bereits erreicht → Fertig");
                Ok(true)
            }
            Ok(false) => {
                self.log("Wiederanbindung: führe dieselbe Paketliste erneut aus");
                Ok(false)
            }
            Err(f) => {
                self.log(&format!(
                    "Wiederanbindung: Zustandsprüfung fehlgeschlagen: {}",
                    fehlertexte::kurz(&f)
                ));
                Err(Zustand::Fehlgeschlagen(f))
            }
        }
    }

    /// Prüft über lesende Aufrufe, ob die Pakete schon im Zielzustand sind:
    /// Installieren → alle installiert; Entfernen → keins mehr installiert;
    /// Aktualisieren → keins mehr in der Aktualisierungsliste;
    /// QuellenAuffrischen → nie (wird wiederholt, ist harmlos).
    async fn ziel_erreicht(&self, vorgang: &Vorgang, ids: &[PaketId]) -> Result<bool, Fehler> {
        let namen: Vec<&str> = ids
            .iter()
            .map(|id| id.split(';').next().unwrap_or(id))
            .collect();
        match vorgang {
            Vorgang::Installieren | Vorgang::Entfernen => {
                let soll = *vorgang == Vorgang::Installieren;
                for n in namen {
                    let ist = self
                        .quelle
                        .paket_nach_name(n)
                        .await?
                        .map(|p| p.installiert)
                        .unwrap_or(false);
                    if ist != soll {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Vorgang::Aktualisieren => {
                let offen = self.quelle.aktualisierungen().await?;
                Ok(!offen.iter().any(|a| namen.contains(&a.paket.name.as_str())))
            }
            Vorgang::QuellenAuffrischen => Ok(false),
        }
    }

    /// Schläft `dauer`; `false`, wenn währenddessen abgebrochen wurde.
    async fn schlafen_oder_abbruch(&self, dauer: Duration) -> bool {
        if self.abbruch_gewuenscht.get() {
            return false;
        }
        let mut wecker = self.schlafen(dauer);
        loop {
            select! {
                _ = wecker => return true,
                b = self.naechster_befehl().fuse() => match b {
                    Some(Befehl::Abbrechen) | None => return false,
                    _ => {}
                },
            }
        }
    }

    // -------------------------------------------------------------------
    // Beobachter nach App-Neustart (`wieder_anbinden`)
    // -------------------------------------------------------------------

    /// Beobachtet laufende Transaktionen des Daemons, bis keine mehr läuft.
    /// Die Schnittstelle erlaubt kein Anbinden an einen Objektpfad, daher nur
    /// Beobachten: `Laeuft` solange etwas läuft, dann `Fertig`; `abbrechen()`
    /// beendet nur das Beobachten (`Abgebrochen`), nicht die fremde Transaktion.
    async fn beobachten(&self) {
        let _aufraeumer = Aufraeumer {
            innen: self.innen.clone(),
            id: self.id,
        };
        let e = &self.innen.einstellungen;
        loop {
            match self.quelle.laufende_transaktionen().await {
                Err(f) => {
                    self.log(&format!(
                        "Wiederanbindung: Daemon nicht erreichbar: {}",
                        fehlertexte::kurz(&f)
                    ));
                    self.melde(Zustand::Fehlgeschlagen(f));
                    return;
                }
                Ok(l) if l.is_empty() => {
                    self.log("Wiederanbindung: keine laufende Transaktion");
                    self.melde(Zustand::Fertig);
                    return;
                }
                Ok(l) => {
                    self.melde(Zustand::Laeuft {
                        prozent: None,
                        status: if l.len() == 1 {
                            "Eine frühere Transaktion der Paketverwaltung läuft noch …".to_string()
                        } else {
                            format!(
                                "{} frühere Transaktionen der Paketverwaltung laufen noch …",
                                l.len()
                            )
                        },
                    });
                }
            }
            if !self.schlafen_oder_abbruch(e.daemon_wartezeit).await {
                self.log("Wiederanbindung: Beobachten beendet");
                self.melde(Zustand::Abgebrochen);
                return;
            }
        }
    }
}

struct Sperre {
    versuche: u32,
    hoechstens: u32,
}

impl Sperre {
    fn neu(e: &Einstellungen) -> Self {
        Self {
            versuche: 0,
            hoechstens: e.sperre_versuche(),
        }
    }
}

/// Was zählt fürs Protokoll als „anderer Zustand“: die Art, bei `Laeuft` der
/// Statustext ohne Paketzusatz („… – vlc 40 %“) und ohne Prozent.
fn protokollschluessel(z: &Zustand) -> String {
    match z {
        Zustand::Laeuft { status, .. } => {
            let basis = status.split(" – ").next().unwrap_or(status);
            format!("Laeuft:{basis}")
        }
        Zustand::WartetAufSperre { .. } => "WartetAufSperre".to_string(),
        anders => protokoll::zustand_kurz(anders),
    }
}

fn zusammen(status: &str, paket_info: &str) -> String {
    if paket_info.is_empty() {
        status.to_string()
    } else if status.is_empty() {
        paket_info.to_string()
    } else {
        format!("{status} – {paket_info}")
    }
}

/// Bestätigung nötig? Entfernen immer; Installieren, wenn die Simulation
/// zusätzliche Pakete oder mehr als die Downloadgrenze zeigt. Die angefragten
/// Pakete selbst zählen nicht als zusätzlich (SW-1 lässt sie in der Simulation
/// ohnehin weg; sollte ein Backend sie mitliefern, werden sie hier gefiltert).
pub fn braucht_bestaetigung(e: &Einstellungen, auftrag: &Auftrag, sim: &Simulation) -> bool {
    match auftrag.vorgang {
        Vorgang::Entfernen => true,
        Vorgang::Installieren => {
            let zusatz = sim.installieren.iter().any(|p| {
                !auftrag
                    .pakete
                    .iter()
                    .any(|a| a.id == p.id || a.name == p.name)
            });
            zusatz
                || !sim.entfernen.is_empty()
                || sim.download_bytes > e.bestaetigung_ab_download_bytes
        }
        Vorgang::Aktualisieren | Vorgang::QuellenAuffrischen => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::typen::Paket;

    fn paket(name: &str) -> Paket {
        Paket {
            id: format!("{name};1.0;amd64;main"),
            name: name.to_string(),
            version: "1.0".into(),
            arch: "amd64".into(),
            zusammenfassung: String::new(),
            installiert: false,
        }
    }

    fn auftrag(v: Vorgang, namen: &[&str]) -> Auftrag {
        Auftrag {
            vorgang: v,
            pakete: namen.iter().map(|n| paket(n)).collect(),
            anwendung: None,
        }
    }

    #[test]
    fn bestaetigung_entfernen_immer() {
        let e = Einstellungen::default();
        assert!(braucht_bestaetigung(
            &e,
            &auftrag(Vorgang::Entfernen, &["a"]),
            &Simulation::default()
        ));
    }

    #[test]
    fn bestaetigung_installieren_nur_mit_zusatz_oder_groesse() {
        let e = Einstellungen::default();
        let a = auftrag(Vorgang::Installieren, &["a"]);
        let nur_a = Simulation {
            installieren: vec![paket("a")],
            download_bytes: 1024,
            ..Default::default()
        };
        assert!(!braucht_bestaetigung(&e, &a, &nur_a));
        let mit_b = Simulation {
            installieren: vec![paket("a"), paket("b")],
            ..Default::default()
        };
        assert!(braucht_bestaetigung(&e, &a, &mit_b));
        let gross = Simulation {
            installieren: vec![paket("a")],
            download_bytes: 51 * 1024 * 1024,
            ..Default::default()
        };
        assert!(braucht_bestaetigung(&e, &a, &gross));
        let genau = Simulation {
            installieren: vec![paket("a")],
            download_bytes: 50 * 1024 * 1024,
            ..Default::default()
        };
        assert!(!braucht_bestaetigung(&e, &a, &genau));
    }

    #[test]
    fn bestaetigung_aktualisieren_nie() {
        let e = Einstellungen::default();
        let s = Simulation {
            aktualisieren: vec![paket("a"), paket("b")],
            download_bytes: 900 * 1024 * 1024,
            ..Default::default()
        };
        assert!(!braucht_bestaetigung(
            &e,
            &auftrag(Vorgang::Aktualisieren, &["a"]),
            &s
        ));
    }

    #[test]
    fn sperrversuche_aus_dauern() {
        let e = Einstellungen::default();
        assert_eq!(e.sperre_versuche(), 30);
        let kurz = Einstellungen {
            sperre_wiederholung: Duration::from_millis(7),
            sperre_hoechstdauer: Duration::from_millis(20),
            ..Default::default()
        };
        assert_eq!(kurz.sperre_versuche(), 3);
    }

    #[test]
    fn griff_ohne_transaktion_ist_wirkungslos() {
        let v: Verzeichnis = Rc::new(RefCell::new(HashMap::new()));
        let g = griff_bauen(&v, u64::MAX);
        g.abbrechen();
        g.bestaetigen();
        g.sperre_erneut_versuchen();
        assert_eq!(g.id, u64::MAX);
    }
}
