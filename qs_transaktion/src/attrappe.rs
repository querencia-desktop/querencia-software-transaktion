//! [`AttrappeQuelle`]: schnelle, deterministische Implementierung von
//! [`Paketquelle`] mit einstellbaren Fehlern und Verzögerungen.
//!
//! Sie dient den Unit-Tests der Zustandsmaschine und `qs-cli --attrappe`. Sie
//! verändert nie etwas am System; „installieren“ heißt hier: das Merkmal
//! `installiert` im eigenen Katalog umschalten. Dadurch sieht die
//! Wiederanbindung über `paket_nach_name` den neuen Zustand – wie beim echten
//! Daemon. Die Simulation nennt wie bei SW-1 nur die *zusätzlichen* Pakete.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use crate::laufzeit::Laufzeit;
use crate::typen::{
    Abbruch, Aktualisierung, Dringlichkeit, Fehler, Fortschritt, Paket, PaketId, Paketdetails,
    Paketquelle, Simulation, Vorgang,
};

/// Zähler der Aufrufe, für Zusicherungen in Tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Aufrufe {
    pub suchen: u32,
    pub details: u32,
    pub paket_nach_name: u32,
    pub aktualisierungen: u32,
    pub simulieren: u32,
    pub ausfuehren: u32,
    pub laufende_transaktionen: u32,
    pub abbrechen: u32,
}

struct Innen {
    katalog: Vec<Paket>,
    aktualisierungen: Vec<Aktualisierung>,
    zusatz_installieren: Vec<Paket>,
    zusatz_entfernen: Vec<Paket>,
    download_bytes: u64,
    schritte: u8,
    schrittdauer: Duration,
    simulationsdauer: Duration,
    fehler_simulation: VecDeque<Fehler>,
    fehler_simulation_dauerhaft: Option<Fehler>,
    fehler_ausfuehren: VecDeque<Fehler>,
    fehler_ausfuehren_dauerhaft: Option<Fehler>,
    /// Bei welchem Schritt der Fehler kommt (1 = nach der ersten Fortschrittsmeldung).
    fehlerschritt: u8,
    /// Änderung am Katalog schon vor dem Fehler anwenden (Daemon stirbt nach dpkg).
    wirkung_vor_fehler: bool,
    braucht_neustart: bool,
    haengen: bool,
    abbruch_ignorieren: bool,
    abbruch_fehler: Option<Fehler>,
    skript: Option<Vec<Fortschritt>>,
    laufende: VecDeque<Result<Vec<String>, Fehler>>,
    laufende_dauerhaft: Result<Vec<String>, Fehler>,
    aufrufe: Aufrufe,
    /// Alle ausgeführten (vorgang, ids), für Zusicherungen.
    ausgefuehrt: Vec<(Vorgang, Vec<PaketId>)>,
}

/// Siehe Modul-Dokumentation. Erzeugen mit [`AttrappeQuelle::neu`] oder
/// [`AttrappeQuelle::beispiel`], dann mit den `mit_…`/`fehler_…`-Methoden einstellen.
pub struct AttrappeQuelle {
    laufzeit: Rc<dyn Laufzeit>,
    innen: Rc<RefCell<Innen>>,
}

/// Baut ein Paket im PackageKit-Format.
pub fn paket(name: &str, version: &str, zusammenfassung: &str, installiert: bool) -> Paket {
    Paket {
        id: format!(
            "{name};{version};amd64;{}",
            if installiert { "installed" } else { "noble" }
        ),
        name: name.to_string(),
        version: version.to_string(),
        arch: "amd64".to_string(),
        zusammenfassung: zusammenfassung.to_string(),
        installiert,
    }
}

impl AttrappeQuelle {
    /// Leere Attrappe: kein Katalog, alle Vorgänge gelingen in 4 Schritten à 10 ms.
    pub fn neu(laufzeit: Rc<dyn Laufzeit>) -> Self {
        Self {
            laufzeit,
            innen: Rc::new(RefCell::new(Innen {
                katalog: Vec::new(),
                aktualisierungen: Vec::new(),
                zusatz_installieren: Vec::new(),
                zusatz_entfernen: Vec::new(),
                download_bytes: 0,
                schritte: 4,
                schrittdauer: Duration::from_millis(10),
                simulationsdauer: Duration::from_millis(5),
                fehler_simulation: VecDeque::new(),
                fehler_simulation_dauerhaft: None,
                fehler_ausfuehren: VecDeque::new(),
                fehler_ausfuehren_dauerhaft: None,
                fehlerschritt: 1,
                wirkung_vor_fehler: false,
                braucht_neustart: false,
                haengen: false,
                abbruch_ignorieren: false,
                abbruch_fehler: None,
                skript: None,
                laufende: VecDeque::new(),
                laufende_dauerhaft: Ok(Vec::new()),
                aufrufe: Aufrufe::default(),
                ausgefuehrt: Vec::new(),
            })),
        }
    }

    /// Attrappe mit Beispielkatalog (12 Pakete, 2 Aktualisierungen) für `qs-cli`.
    pub fn beispiel(laufzeit: Rc<dyn Laufzeit>) -> Self {
        let a = Self::neu(laufzeit).mit_paketen(vec![
            paket("gimp", "2.10.38-1", "GNU-Bildbearbeitungsprogramm", false),
            paket("inkscape", "1.4-1", "Vektorgrafik-Editor", false),
            paket("vlc", "3.0.21-1", "Multimedia-Spieler und -Streamer", false),
            paket("gedit", "48.1-1", "Texteditor für GNOME", false),
            paket(
                "libreoffice-writer",
                "4:25.2.1-1",
                "Textverarbeitung",
                false,
            ),
            paket("firefox", "128.0-1", "Webbrowser", false),
            paket(
                "querencia-bilder",
                "0.3.1",
                "Bildbetrachter für Querencia Desktop",
                true,
            ),
            paket(
                "querencia-editor",
                "0.4.0",
                "Texteditor für Querencia Desktop",
                true,
            ),
            paket(
                "querencia-rechner",
                "0.1.0",
                "Rechner für Querencia Desktop",
                true,
            ),
            paket(
                "querencia-desktop",
                "0.9.2",
                "Arbeitsfläche von Querencia Desktop",
                true,
            ),
            paket("labwc", "0.9.8-1", "Wayland-Fensterverwalter", true),
            paket("htop", "3.3.0-4", "Interaktiver Prozessbetrachter", true),
        ]);
        a.innen.borrow_mut().aktualisierungen = vec![
            Aktualisierung {
                paket: paket("labwc", "0.9.9-1", "Wayland-Fensterverwalter", false),
                dringlichkeit: Dringlichkeit::Fehlerbehebung,
                neustart_noetig: true,
            },
            Aktualisierung {
                paket: paket("htop", "3.4.0-1", "Interaktiver Prozessbetrachter", false),
                dringlichkeit: Dringlichkeit::Normal,
                neustart_noetig: false,
            },
        ];
        a.innen.borrow_mut().download_bytes = 12 * 1024 * 1024;
        a
    }

    // ---- Einstellungen (verbrauchen `self`, Builder-Stil) -------------------

    pub fn mit_paketen(self, pakete: Vec<Paket>) -> Self {
        self.innen.borrow_mut().katalog = pakete;
        self
    }

    /// Zusätzliche Pakete, die die Simulation bei Installieren zeigt.
    pub fn mit_zusatz_installieren(self, pakete: Vec<Paket>) -> Self {
        self.innen.borrow_mut().zusatz_installieren = pakete;
        self
    }

    /// Zusätzliche Pakete, die die Simulation bei Entfernen zeigt (gehen mit).
    pub fn mit_zusatz_entfernen(self, pakete: Vec<Paket>) -> Self {
        self.innen.borrow_mut().zusatz_entfernen = pakete;
        self
    }

    pub fn mit_download_bytes(self, bytes: u64) -> Self {
        self.innen.borrow_mut().download_bytes = bytes;
        self
    }

    /// Zahl der Fortschrittsschritte und Dauer je Schritt.
    pub fn mit_schritten(self, schritte: u8, dauer: Duration) -> Self {
        let mut i = self.innen.borrow_mut();
        i.schritte = schritte.max(1);
        i.schrittdauer = dauer;
        drop(i);
        self
    }

    pub fn mit_simulationsdauer(self, dauer: Duration) -> Self {
        self.innen.borrow_mut().simulationsdauer = dauer;
        self
    }

    /// Die ersten `fehler.len()` Simulationen scheitern mit diesen Fehlern, danach gelingen sie.
    pub fn fehler_bei_simulation(self, fehler: Vec<Fehler>) -> Self {
        self.innen.borrow_mut().fehler_simulation = fehler.into();
        self
    }

    pub fn fehler_bei_simulation_dauerhaft(self, fehler: Fehler) -> Self {
        self.innen.borrow_mut().fehler_simulation_dauerhaft = Some(fehler);
        self
    }

    /// Die ersten `fehler.len()` Ausführungen scheitern mit diesen Fehlern, danach gelingen sie.
    pub fn fehler_bei_ausfuehren(self, fehler: Vec<Fehler>) -> Self {
        self.innen.borrow_mut().fehler_ausfuehren = fehler.into();
        self
    }

    /// Jede Ausführung scheitert mit diesem Fehler.
    pub fn fehler_bei_ausfuehren_dauerhaft(self, fehler: Fehler) -> Self {
        self.innen.borrow_mut().fehler_ausfuehren_dauerhaft = Some(fehler);
        self
    }

    /// Bei welchem Schritt der Fehler kommt (0 = sofort nach `Gestartet`).
    pub fn mit_fehlerschritt(self, schritt: u8) -> Self {
        self.innen.borrow_mut().fehlerschritt = schritt;
        self
    }

    /// Die Änderung am Katalog wird vor dem Fehler angewendet (modelliert einen
    /// Daemon, der nach dpkg stirbt: die zweite Simulation zeigt nichts mehr).
    pub fn wirkung_vor_fehler(self) -> Self {
        self.innen.borrow_mut().wirkung_vor_fehler = true;
        self
    }

    pub fn mit_neustart(self) -> Self {
        self.innen.borrow_mut().braucht_neustart = true;
        self
    }

    /// Nach `Gestartet` kommt nie mehr eine Meldung (bis zum Abbruch).
    pub fn haengend(self) -> Self {
        self.innen.borrow_mut().haengen = true;
        self
    }

    /// `abbrechen()` wird ignoriert (Quelle bestätigt den Abbruch nie).
    pub fn abbruch_ignorieren(self) -> Self {
        self.innen.borrow_mut().abbruch_ignorieren = true;
        self
    }

    /// Nach `abbrechen()` meldet die Quelle nicht `Abgebrochen`, sondern diesen
    /// Fehler (so verhält sich SW-1s Prüf-Attrappe bei belegter Sperre).
    pub fn abbruch_meldet_fehler(self, fehler: Fehler) -> Self {
        self.innen.borrow_mut().abbruch_fehler = Some(fehler);
        self
    }

    /// Skriptmodus: `ausfuehren` meldet nach `Gestartet` genau diese Folge (je
    /// eine Schrittdauer Abstand) und endet dann – auch ohne Endmeldung, dann
    /// schließt sich der Meldekanal (wie ein verschwundener Daemon). Enthält die
    /// Folge `Fertig`, wird der Vorgang auf den Katalog angewendet.
    pub fn mit_skript(self, folge: Vec<Fortschritt>) -> Self {
        self.innen.borrow_mut().skript = Some(folge);
        self
    }

    /// Antworten auf `laufende_transaktionen`, in dieser Reihenfolge; danach `dauerhaft`.
    pub fn laufende_antworten(self, antworten: Vec<Result<Vec<String>, Fehler>>) -> Self {
        self.innen.borrow_mut().laufende = antworten.into();
        self
    }

    pub fn laufende_dauerhaft(self, antwort: Result<Vec<String>, Fehler>) -> Self {
        self.innen.borrow_mut().laufende_dauerhaft = antwort;
        self
    }

    // ---- Abfragen -------------------------------------------------------------

    pub fn aufrufe(&self) -> Aufrufe {
        self.innen.borrow().aufrufe.clone()
    }

    /// Alle Aufrufe von `ausfuehren` (Vorgang, IDs), in Reihenfolge.
    pub fn ausgefuehrt(&self) -> Vec<(Vorgang, Vec<PaketId>)> {
        self.innen.borrow().ausgefuehrt.clone()
    }

    pub fn ist_installiert(&self, name: &str) -> bool {
        self.innen
            .borrow()
            .katalog
            .iter()
            .any(|p| p.name == name && p.installiert)
    }

    pub fn katalog(&self) -> Vec<Paket> {
        self.innen.borrow().katalog.clone()
    }

    fn schlafen(&self, d: Duration) -> crate::laufzeit::Aufgabe {
        self.laufzeit.schlafen(d)
    }
}

impl Innen {
    fn finde(&self, id: &str) -> Option<Paket> {
        let name = id.split(';').next().unwrap_or(id);
        self.katalog
            .iter()
            .find(|p| p.id == id)
            .or_else(|| self.katalog.iter().find(|p| p.name == name))
            .cloned()
    }

    fn simulation(&self, vorgang: &Vorgang, ids: &[PaketId]) -> Simulation {
        let mut sim = Simulation::default();
        let gewaehlt: Vec<Paket> = ids
            .iter()
            .map(|id| {
                self.finde(id).unwrap_or_else(|| {
                    paket(
                        id.split(';').next().unwrap_or(id),
                        "0",
                        "(unbekannt)",
                        false,
                    )
                })
            })
            .collect();
        match vorgang {
            // Wie SW-1: die Simulation nennt nur, was *zusätzlich* geschieht;
            // die angefragten Pakete selbst erscheinen nicht.
            Vorgang::Installieren => {
                if gewaehlt.iter().any(|p| !p.installiert) {
                    sim.installieren = self
                        .zusatz_installieren
                        .iter()
                        .filter(|z| {
                            !self
                                .katalog
                                .iter()
                                .any(|k| k.name == z.name && k.installiert)
                        })
                        .cloned()
                        .collect();
                    sim.download_bytes = self.download_bytes;
                }
            }
            Vorgang::Entfernen => {
                if gewaehlt.iter().any(|p| p.installiert) {
                    sim.entfernen = self.zusatz_entfernen.clone();
                }
            }
            Vorgang::Aktualisieren => {
                sim.aktualisieren = gewaehlt
                    .into_iter()
                    .filter(|p| self.aktualisierungen.iter().any(|a| a.paket.name == p.name))
                    .collect();
                sim.download_bytes = self.download_bytes;
            }
            Vorgang::QuellenAuffrischen => {}
        }
        sim
    }

    /// Wendet den Vorgang auf den Katalog an.
    fn anwenden(&mut self, vorgang: &Vorgang, ids: &[PaketId]) {
        let namen: Vec<String> = ids
            .iter()
            .map(|id| id.split(';').next().unwrap_or(id).to_string())
            .collect();
        match vorgang {
            Vorgang::Installieren => {
                for n in &namen {
                    if let Some(p) = self.katalog.iter_mut().find(|p| &p.name == n) {
                        p.installiert = true;
                    } else {
                        self.katalog
                            .push(paket(n, "0", "(installiert über Attrappe)", true));
                    }
                }
                for z in &self.zusatz_installieren {
                    if let Some(p) = self.katalog.iter_mut().find(|p| p.name == z.name) {
                        p.installiert = true;
                    }
                }
            }
            Vorgang::Entfernen => {
                for n in &namen {
                    if let Some(p) = self.katalog.iter_mut().find(|p| &p.name == n) {
                        p.installiert = false;
                    }
                }
            }
            Vorgang::Aktualisieren => {
                for n in &namen {
                    self.aktualisierungen.retain(|a| &a.paket.name != n);
                }
            }
            Vorgang::QuellenAuffrischen => {}
        }
    }

    fn naechster_fehler_ausfuehren(&mut self) -> Option<Fehler> {
        self.fehler_ausfuehren
            .pop_front()
            .or_else(|| self.fehler_ausfuehren_dauerhaft.clone())
    }

    fn naechster_fehler_simulation(&mut self) -> Option<Fehler> {
        self.fehler_simulation
            .pop_front()
            .or_else(|| self.fehler_simulation_dauerhaft.clone())
    }
}

struct AttrappeAbbruch {
    flagge: Rc<Cell<bool>>,
    innen: Rc<RefCell<Innen>>,
}

impl Abbruch for AttrappeAbbruch {
    fn abbrechen(&self) {
        self.innen.borrow_mut().aufrufe.abbrechen += 1;
        self.flagge.set(true);
    }
}

#[async_trait::async_trait(?Send)]
impl Paketquelle for AttrappeQuelle {
    async fn suchen(&self, text: &str, nur_installiert: bool) -> Result<Vec<Paket>, Fehler> {
        let dauer = {
            let mut i = self.innen.borrow_mut();
            i.aufrufe.suchen += 1;
            i.simulationsdauer
        };
        self.schlafen(dauer).await;
        let t = text.to_lowercase();
        Ok(self
            .innen
            .borrow()
            .katalog
            .iter()
            .filter(|p| !nur_installiert || p.installiert)
            .filter(|p| {
                t.is_empty()
                    || p.name.to_lowercase().contains(&t)
                    || p.zusammenfassung.to_lowercase().contains(&t)
            })
            .cloned()
            .collect())
    }

    async fn paket_nach_name(&self, name: &str) -> Result<Option<Paket>, Fehler> {
        let dauer = {
            let mut i = self.innen.borrow_mut();
            i.aufrufe.paket_nach_name += 1;
            i.simulationsdauer
        };
        self.schlafen(dauer).await;
        let i = self.innen.borrow();
        let mut treffer: Vec<&Paket> = i.katalog.iter().filter(|p| p.name == name).collect();
        treffer.sort_by_key(|p| !p.installiert);
        Ok(treffer.first().cloned().cloned())
    }

    async fn details(&self, id: &PaketId) -> Result<Paketdetails, Fehler> {
        let dauer = {
            let mut i = self.innen.borrow_mut();
            i.aufrufe.details += 1;
            i.simulationsdauer
        };
        self.schlafen(dauer).await;
        let i = self.innen.borrow();
        let p = i
            .finde(id)
            .ok_or_else(|| Fehler::PaketNichtGefunden(id.clone()))?;
        Ok(Paketdetails {
            beschreibung: format!(
                "{}\n\nDies ist ein Beispielpaket der Attrappe; es wird nichts installiert.",
                p.zusammenfassung
            ),
            lizenz: "GPL-3.0-or-later".to_string(),
            groesse_bytes: i.download_bytes.max(1024 * 512),
            url: Some(format!("https://example.invalid/{}", p.name)),
            gruppe: "Sonstiges".to_string(),
            paket: p,
        })
    }

    async fn aktualisierungen(&self) -> Result<Vec<Aktualisierung>, Fehler> {
        let dauer = {
            let mut i = self.innen.borrow_mut();
            i.aufrufe.aktualisierungen += 1;
            i.simulationsdauer
        };
        self.schlafen(dauer).await;
        Ok(self.innen.borrow().aktualisierungen.clone())
    }

    fn ausfuehren(
        &self,
        vorgang: Vorgang,
        ids: Vec<PaketId>,
        melden: Box<dyn Fn(Fortschritt)>,
    ) -> Box<dyn Abbruch> {
        let flagge = Rc::new(Cell::new(false));
        let innen = self.innen.clone();
        let laufzeit = self.laufzeit.clone();
        let skript = innen.borrow().skript.clone();
        let abbruch_fehler = innen.borrow().abbruch_fehler.clone();
        let abbruch_meldung = move || match &abbruch_fehler {
            Some(f) => Fortschritt::Fehler(f.clone()),
            None => Fortschritt::Abgebrochen,
        };
        let (
            fehler,
            schritte,
            dauer,
            fehlerschritt,
            wirkung_vor_fehler,
            neustart,
            haengen,
            ignorieren,
        ) = {
            let mut i = innen.borrow_mut();
            i.aufrufe.ausfuehren += 1;
            i.ausgefuehrt.push((vorgang.clone(), ids.clone()));
            (
                i.naechster_fehler_ausfuehren(),
                i.schritte,
                i.schrittdauer,
                i.fehlerschritt,
                i.wirkung_vor_fehler,
                i.braucht_neustart,
                i.haengen,
                i.abbruch_ignorieren,
            )
        };
        let f = flagge.clone();
        let innen2 = innen.clone();
        laufzeit.clone().starten(Box::pin(async move {
            melden(Fortschritt::Gestartet {
                vorgang: vorgang.clone(),
                pakete: ids.clone(),
            });
            if let Some(folge) = skript {
                for f_meldung in folge {
                    laufzeit.schlafen(dauer).await;
                    if f.get() && !ignorieren {
                        melden(abbruch_meldung());
                        return;
                    }
                    if f_meldung == Fortschritt::Fertig {
                        innen2.borrow_mut().anwenden(&vorgang, &ids);
                    }
                    melden(f_meldung);
                }
                return;
            }
            if haengen {
                loop {
                    laufzeit.schlafen(Duration::from_secs(1)).await;
                    if f.get() && !ignorieren {
                        melden(abbruch_meldung());
                        return;
                    }
                }
            }
            if let (Some(fehler), 0) = (&fehler, fehlerschritt) {
                if wirkung_vor_fehler {
                    innen2.borrow_mut().anwenden(&vorgang, &ids);
                }
                melden(Fortschritt::Fehler(fehler.clone()));
                return;
            }
            let texte = [
                "Lade herunter …",
                "Bereite vor …",
                "Richte ein …",
                "Räume auf …",
            ];
            for s in 1..=schritte {
                laufzeit.schlafen(dauer).await;
                if f.get() && !ignorieren {
                    melden(abbruch_meldung());
                    return;
                }
                melden(Fortschritt::Status(
                    texte[((s as usize) - 1).min(texte.len() - 1)].to_string(),
                ));
                if let Some(id) = ids.first() {
                    melden(Fortschritt::PaketFortschritt {
                        id: id.clone(),
                        prozent: ((s as u32) * 100 / schritte as u32) as u8,
                    });
                }
                melden(Fortschritt::Prozent(
                    ((s as u32) * 100 / schritte as u32) as u8,
                ));
                if let Some(fehler) = &fehler {
                    if s == fehlerschritt {
                        if wirkung_vor_fehler {
                            innen2.borrow_mut().anwenden(&vorgang, &ids);
                        }
                        melden(Fortschritt::Fehler(fehler.clone()));
                        return;
                    }
                }
            }
            if let Some(fehler) = &fehler {
                // Fehlerschritt größer als Schrittzahl: am Ende scheitern.
                if wirkung_vor_fehler {
                    innen2.borrow_mut().anwenden(&vorgang, &ids);
                }
                melden(Fortschritt::Fehler(fehler.clone()));
                return;
            }
            innen2.borrow_mut().anwenden(&vorgang, &ids);
            if neustart {
                melden(Fortschritt::BrauchtNeustart);
            }
            melden(Fortschritt::Fertig);
        }));
        Box::new(AttrappeAbbruch { flagge, innen })
    }

    async fn simulieren(&self, vorgang: Vorgang, ids: Vec<PaketId>) -> Result<Simulation, Fehler> {
        let (fehler, dauer) = {
            let mut i = self.innen.borrow_mut();
            i.aufrufe.simulieren += 1;
            (i.naechster_fehler_simulation(), i.simulationsdauer)
        };
        self.schlafen(dauer).await;
        if let Some(f) = fehler {
            return Err(f);
        }
        Ok(self.innen.borrow().simulation(&vorgang, &ids))
    }

    async fn laufende_transaktionen(&self) -> Result<Vec<String>, Fehler> {
        let antwort = {
            let mut i = self.innen.borrow_mut();
            i.aufrufe.laufende_transaktionen += 1;
            i.laufende
                .pop_front()
                .unwrap_or_else(|| i.laufende_dauerhaft.clone())
        };
        self.schlafen(Duration::from_millis(1)).await;
        antwort
    }
}
