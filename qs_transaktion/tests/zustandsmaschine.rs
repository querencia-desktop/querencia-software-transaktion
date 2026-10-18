//! Unit-Tests der Zustandsmaschine gegen die `AttrappeQuelle`.
//!
//! Alle Tests laufen auf einem tokio-Ein-Thread-Laufzeit mit angehaltener Uhr
//! (`start_paused`): Wartezeiten von Minuten vergehen sofort, aber in der
//! richtigen Reihenfolge. Zeiten werden mit `tokio::time::Instant` gemessen.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use qs_transaktion::attrappe::{paket, AttrappeQuelle};
use qs_transaktion::laufzeit::TokioLaufzeit;
use qs_transaktion::{
    ist_ende, ist_nachfrage, Auftrag, Einstellungen, Fehler, Fortschritt, Laufzeit, Monitor,
    Paket, Paketquelle, Protokoll, TransaktionsGriff, Transaktionsmonitor, Vorgang, Zustand,
};
use tokio::task::LocalSet;
use tokio::time::Instant;

// ---------------------------------------------------------------------------
// Hilfen
// ---------------------------------------------------------------------------

type Liste = Rc<RefCell<Vec<Zustand>>>;

fn laufzeit() -> Rc<dyn Laufzeit> {
    Rc::new(TokioLaufzeit)
}

fn monitor() -> Monitor {
    monitor_mit(Einstellungen::default())
}

fn monitor_mit(e: Einstellungen) -> Monitor {
    Monitor::mit(laufzeit(), e, Rc::new(Protokoll::stumm()))
}

fn katalog() -> Vec<Paket> {
    vec![
        paket("gimp", "2.10", "Bildbearbeitung", false),
        paket("gimp-data", "2.10", "Daten für GIMP", false),
        paket("vlc", "3.0", "Medienspieler", false),
        paket("inkscape", "1.4", "Vektorgrafik", false),
        paket("htop", "3.3", "Prozessbetrachter", true),
        paket("libhtop", "3.3", "Bibliothek", true),
    ]
}

fn quelle() -> AttrappeQuelle {
    AttrappeQuelle::neu(laufzeit()).mit_paketen(katalog())
}

fn auftrag(v: Vorgang, name: &str) -> Auftrag {
    let p = katalog().into_iter().find(|p| p.name == name).unwrap();
    Auftrag {
        vorgang: v,
        pakete: vec![p],
        anwendung: None,
    }
}

fn melder() -> (Liste, Box<dyn Fn(Zustand)>) {
    let l: Liste = Rc::new(RefCell::new(Vec::new()));
    let l2 = l.clone();
    (l, Box::new(move |z| l2.borrow_mut().push(z)))
}

/// Startet einen Auftrag; liefert Griff, Zustandsliste und die Quelle als Rc.
fn starte(m: &Monitor, q: &Rc<AttrappeQuelle>, a: Auftrag) -> (TransaktionsGriff, Liste) {
    let (l, f) = melder();
    let g = m.starten(a, q.clone() as Rc<dyn Paketquelle>, f);
    (g, l)
}

/// Wartet (virtuelle Zeit), bis `pred` auf den letzten Zustand zutrifft; höchstens 2 h.
async fn warte_bis(l: &Liste, pred: impl Fn(&Zustand) -> bool) {
    let ende = Instant::now() + Duration::from_secs(7200);
    loop {
        if l.borrow().last().map(&pred).unwrap_or(false) {
            return;
        }
        assert!(Instant::now() < ende, "Zeitüberschreitung; Zustände: {:?}", l.borrow());
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn warte_ende(l: &Liste) -> Zustand {
    warte_bis(l, ist_ende).await;
    // Kurz weiterlaufen lassen: nach dem Ende darf nichts mehr kommen.
    let n = l.borrow().len();
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(l.borrow().len(), n, "Meldung nach Endzustand: {:?}", l.borrow());
    l.borrow().last().cloned().unwrap()
}

fn hat(l: &Liste, pred: impl Fn(&Zustand) -> bool) -> bool {
    l.borrow().iter().any(pred)
}

fn ist_laeuft(z: &Zustand) -> bool {
    matches!(z, Zustand::Laeuft { .. })
}

fn ist_sperre(z: &Zustand) -> bool {
    matches!(z, Zustand::WartetAufSperre { .. })
}

fn ist_bestaetigung(z: &Zustand) -> bool {
    matches!(z, Zustand::WartetAufBestaetigung)
}

fn sonstiges(code: &str) -> Fehler {
    Fehler::Sonstiges {
        code: code.into(),
        text: "Test".into(),
    }
}

/// Ein-Thread-Laufzeit mit angehaltener Uhr und `LocalSet` (Ersatz für
/// `#[tokio::test(start_paused = true)]`, da `tokio-macros` nicht im Cache liegt).
fn ausfuehren_lokal<F: std::future::Future<Output = ()>>(f: F) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("tokio-Laufzeit");
    LocalSet::new().block_on(&rt, f);
}

macro_rules! lokal {
    ($body:expr) => {
        ausfuehren_lokal($body)
    };
}

// ---------------------------------------------------------------------------
// Erfolg und Bestätigung
// ---------------------------------------------------------------------------

#[test]
fn t01_installieren_erfolg_ohne_bestaetigung() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle());
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        let z = l.borrow().clone();
        assert!(matches!(&z[0], Zustand::Simuliert(s) if s.installieren.is_empty()));
        assert!(!z.iter().any(ist_bestaetigung));
        assert!(z.iter().any(|z| matches!(z, Zustand::Laeuft { prozent: Some(100), .. })));
        assert!(q.ist_installiert("vlc"));
        assert_eq!(q.aufrufe().ausfuehren, 1);
        assert_eq!(q.aufrufe().simulieren, 1);
    })
}

#[test]
fn t02_installieren_mit_zusatzpaketen_fragt_nach() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_zusatz_installieren(vec![paket("gimp-data", "2.10", "", false)]));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "gimp"));
        warte_bis(&l, ist_bestaetigung).await;
        assert!(matches!(&l.borrow()[0], Zustand::Simuliert(s) if s.installieren.len() == 1 && s.installieren[0].name == "gimp-data"));
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(q.aufrufe().ausfuehren, 0, "darf ohne Bestätigung nicht ausführen");
        g.bestaetigen();
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(q.ist_installiert("gimp") && q.ist_installiert("gimp-data"));
    })
}

#[test]
fn t03_installieren_ueber_50_mib_fragt_nach() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_download_bytes(80 * 1024 * 1024));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "inkscape"));
        warte_bis(&l, ist_bestaetigung).await;
        g.bestaetigen();
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
    })
}

#[test]
fn t04_entfernen_fragt_immer_nach_und_zeigt_mitgehende() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_zusatz_entfernen(vec![paket("libhtop", "3.3", "", true)]));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Entfernen, "htop"));
        warte_bis(&l, ist_bestaetigung).await;
        assert!(matches!(&l.borrow()[0], Zustand::Simuliert(s) if s.entfernen.len() == 1 && s.entfernen[0].name == "libhtop"));
        g.bestaetigen();
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(!q.ist_installiert("htop"));
        assert_eq!(q.ausgefuehrt()[0].0, Vorgang::Entfernen);
    })
}

#[test]
fn t05_bestaetigung_verweigert() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle());
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Entfernen, "htop"));
        warte_bis(&l, ist_bestaetigung).await;
        g.abbrechen();
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
        assert_eq!(q.aufrufe().ausfuehren, 0);
        assert!(q.ist_installiert("htop"));
    })
}

#[test]
fn t06_vorab_bestaetigung_geht_nicht_verloren() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_simulationsdauer(Duration::from_secs(3)));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Entfernen, "htop"));
        g.bestaetigen(); // trifft während der Simulation ein
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(hat(&l, ist_bestaetigung));
    })
}

#[test]
fn t07_aktualisieren_ohne_bestaetigung() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(AttrappeQuelle::beispiel(laufzeit()).mit_download_bytes(500 * 1024 * 1024));
        let htop = q.katalog().into_iter().find(|p| p.name == "htop").unwrap();
        let (_g, l) = starte(
            &m,
            &q,
            Auftrag { vorgang: Vorgang::Aktualisieren, pakete: vec![htop], anwendung: None },
        );
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(!hat(&l, ist_bestaetigung));
    })
}

#[test]
fn t08_braucht_neustart_merkmal_am_fertig() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_neustart());
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(m.braucht_neustart(g.id));
        let (g2, l2) = starte(&m, &Rc::new(quelle()), auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l2).await, Zustand::Fertig);
        assert!(!m.braucht_neustart(g2.id));
    })
}

#[test]
fn t09_prozent_101_ist_unbekannt_und_status_mit_paket() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_skript(vec![
            Fortschritt::Status("Lade herunter …".into()),
            Fortschritt::Prozent(101),
            Fortschritt::PaketFortschritt { id: "vlc;3.0;amd64;noble".into(), prozent: 40 },
            Fortschritt::Prozent(55),
            Fortschritt::Fertig,
        ]));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(hat(&l, |z| matches!(z, Zustand::Laeuft { prozent: None, status } if status == "Lade herunter …")));
        assert!(hat(&l, |z| matches!(z, Zustand::Laeuft { prozent: Some(55), status } if status == "Lade herunter … – vlc 40 %")));
    })
}

// ---------------------------------------------------------------------------
// Fehler
// ---------------------------------------------------------------------------

#[test]
fn t10_fehler_wie_test_fail() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(sonstiges("internal-error")).mit_fehlerschritt(2));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(sonstiges("internal-error")));
        assert!(hat(&l, ist_laeuft));
        assert_eq!(q.aufrufe().ausfuehren, 1, "kein Wiederholen bei sonstigen Fehlern");
        assert!(!q.ist_installiert("vlc"));
    })
}

#[test]
fn t11_paket_nicht_gefunden_in_simulation() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_simulation(vec![Fehler::PaketNichtGefunden("vlc".into())]));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(Fehler::PaketNichtGefunden("vlc".into())));
        assert_eq!(l.borrow().len(), 1, "nur der Endzustand");
        assert_eq!(q.aufrufe().ausfuehren, 0);
    })
}

#[test]
fn t12_netz_fehlt() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![Fehler::NetzFehlt]));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(Fehler::NetzFehlt));
    })
}

#[test]
fn t13_abhaengigkeitskonflikt() {
    lokal!(async {
        let m = monitor();
        let f = Fehler::AbhaengigkeitenKonflikt("vlc hängt von libvlc5 ab".into());
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![f.clone()]).mit_fehlerschritt(0));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(f));
    })
}

#[test]
fn t14_kein_agent_sofort_fehlgeschlagen() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(Fehler::KeinAgent).mit_fehlerschritt(0));
        let t0 = Instant::now();
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_ende).await;
        assert!(t0.elapsed() < Duration::from_secs(1), "sofort, ohne Wartezeit");
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Fehlgeschlagen(Fehler::KeinAgent));
        assert_eq!(q.aufrufe().ausfuehren, 1);
        let t = qs_transaktion::fehlertexte::fehlertext(&Fehler::KeinAgent);
        assert!(t.erklaerung.contains("polkit-Agent"));
    })
}

#[test]
fn t15_nicht_berechtigt_sofort_fehlgeschlagen() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(Fehler::NichtBerechtigt).mit_fehlerschritt(0));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Entfernen, "htop"));
        warte_bis(&l, ist_bestaetigung).await;
        g.bestaetigen();
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(Fehler::NichtBerechtigt));
        assert_eq!(q.aufrufe().ausfuehren, 1);
        assert!(q.ist_installiert("htop"));
    })
}

#[test]
fn t16_quelle_meldet_abgebrochen_als_fehler() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![Fehler::Abgebrochen]));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
    })
}

// ---------------------------------------------------------------------------
// Sperre
// ---------------------------------------------------------------------------

#[test]
fn t17_sperre_freigabe_nach_zwei_versuchen() {
    lokal!(async {
        let m = monitor();
        let s = Fehler::SperreBelegt("unattended-upgrades".into());
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![s.clone(), s.clone()]).mit_fehlerschritt(0));
        let t0 = Instant::now();
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_ende).await;
        let dauer = t0.elapsed();
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Fertig);
        assert!(hat(&l, ist_sperre));
        assert_eq!(q.aufrufe().ausfuehren, 3);
        assert!(dauer >= Duration::from_secs(20) && dauer < Duration::from_secs(25), "{dauer:?}");
        assert!(q.ist_installiert("vlc"));
    })
}

#[test]
fn t18_sperre_dauerhaft_nach_5_minuten_fehlgeschlagen() {
    lokal!(async {
        let m = monitor();
        let s = Fehler::SperreBelegt("apt in einem Terminal".into());
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(s.clone()).mit_fehlerschritt(0));
        let t0 = Instant::now();
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_ende).await;
        let dauer = t0.elapsed();
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Fehlgeschlagen(s));
        assert!(dauer >= Duration::from_secs(300) && dauer < Duration::from_secs(305), "{dauer:?}");
        assert_eq!(q.aufrufe().ausfuehren, 31, "erster Versuch + 30 Wiederholungen à 10 s");
    })
}

#[test]
fn t19_sperre_bei_simulation_wird_wiederholt() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_simulation(vec![Fehler::SperreBelegt(String::new())]));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(ist_sperre(&l.borrow()[0]));
        assert_eq!(q.aufrufe().simulieren, 2);
    })
}

#[test]
fn t20_sperre_erneut_versuchen_sofort() {
    lokal!(async {
        let m = monitor();
        let s = Fehler::SperreBelegt(String::new());
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![s]).mit_fehlerschritt(0));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_sperre).await;
        let t0 = Instant::now();
        g.sperre_erneut_versuchen();
        warte_bis(&l, ist_ende).await;
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Fertig);
        assert_eq!(q.aufrufe().ausfuehren, 2);
    })
}

#[test]
fn t21_sperre_erneut_versuchen_setzt_frist_zurueck() {
    lokal!(async {
        let m = monitor();
        let s = Fehler::SperreBelegt(String::new());
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(s.clone()).mit_fehlerschritt(0));
        let t0 = Instant::now();
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        tokio::time::sleep(Duration::from_secs(245)).await;
        g.sperre_erneut_versuchen();
        warte_bis(&l, ist_ende).await;
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Fehlgeschlagen(s));
        assert!(t0.elapsed() > Duration::from_secs(500), "{:?}", t0.elapsed());
    })
}

// ---------------------------------------------------------------------------
// Abbruch in jedem Zustand
// ---------------------------------------------------------------------------

#[test]
fn t22_abbruch_waehrend_simulation() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_simulationsdauer(Duration::from_secs(60)));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        tokio::time::sleep(Duration::from_secs(1)).await;
        g.abbrechen();
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
        assert_eq!(l.borrow().len(), 1);
        assert_eq!(q.aufrufe().ausfuehren, 0);
    })
}

#[test]
fn t23_abbruch_in_laeuft() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_schritten(10, Duration::from_secs(5)));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, |z| matches!(z, Zustand::Laeuft { prozent: Some(p), .. } if *p >= 20)).await;
        g.abbrechen();
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
        assert!(hat(&l, |z| matches!(z, Zustand::Laeuft { status, .. } if status == "Wird abgebrochen …")));
        assert_eq!(q.aufrufe().abbrechen, 1);
        assert!(!q.ist_installiert("vlc"));
    })
}

#[test]
fn t24_abbruch_in_laeuft_ohne_rueckmeldung_nach_frist() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().haengend().abbruch_ignorieren());
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_laeuft).await;
        let t0 = Instant::now();
        g.abbrechen();
        g.abbrechen(); // doppelt ist harmlos
        warte_bis(&l, ist_ende).await;
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Abgebrochen);
        let d = t0.elapsed();
        assert!(d >= Duration::from_secs(15) && d < Duration::from_secs(17), "{d:?}");
        assert_eq!(q.aufrufe().abbrechen, 1);
    })
}

#[test]
fn t25_abbruch_waehrend_warten_auf_sperre() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(Fehler::SperreBelegt(String::new())).mit_fehlerschritt(0));
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_sperre).await;
        tokio::time::sleep(Duration::from_secs(35)).await;
        g.abbrechen();
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
        assert!(q.aufrufe().ausfuehren <= 5);
    })
}

#[test]
fn t26_abbruch_in_warteschlange() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_schritten(4, Duration::from_secs(10)));
        let (_g1, l1) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        let (g2, l2) = starte(&m, &q, auftrag(Vorgang::Installieren, "inkscape"));
        warte_bis(&l2, |z| matches!(z, Zustand::Simuliert(_))).await;
        assert_eq!(m.warteschlange(), vec![g2.id]);
        g2.abbrechen();
        assert_eq!(warte_ende(&l2).await, Zustand::Abgebrochen);
        assert_eq!(warte_ende(&l1).await, Zustand::Fertig);
        assert_eq!(q.aufrufe().ausfuehren, 1);
        assert!(!q.ist_installiert("inkscape"));
        assert!(m.warteschlange().is_empty() && m.aktiv().is_none());
    })
}

#[test]
fn t27_abbruch_nach_ende_wirkungslos() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle());
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        g.abbrechen();
        g.bestaetigen();
        g.sperre_erneut_versuchen();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(l.borrow().last().unwrap(), &Zustand::Fertig);
        assert_eq!(m.zustand(g.id), Some(Zustand::Fertig));
    })
}

// ---------------------------------------------------------------------------
// DaemonWeg und Wiederanbindung
// ---------------------------------------------------------------------------

#[test]
fn t28_daemon_weg_wiederanbindung_vorgang_war_schon_fertig() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(
            quelle()
                .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
                .mit_fehlerschritt(3)
                .wirkung_vor_fehler()
                .laufende_antworten(vec![Err(Fehler::DaemonWeg), Ok(vec![])]),
        );
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert_eq!(q.aufrufe().ausfuehren, 1, "nicht doppelt ausführen");
        assert_eq!(q.aufrufe().simulieren, 1);
        assert_eq!(q.aufrufe().paket_nach_name, 1, "Zielzustand über Resolve geprüft");
        assert_eq!(q.aufrufe().laufende_transaktionen, 2);
        assert!(hat(&l, |z| matches!(z, Zustand::Laeuft { status, .. } if status.contains("Wiederanbindung"))));
    })
}

#[test]
fn t29_daemon_weg_wiederanbindung_fuehrt_erneut_aus() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![Fehler::DaemonWeg]).mit_fehlerschritt(1));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert_eq!(q.aufrufe().ausfuehren, 2);
        let a = q.ausgefuehrt();
        assert_eq!(a[0], a[1], "gleiche Paketliste");
        assert!(q.ist_installiert("vlc"));
    })
}

#[test]
fn t30_daemon_weg_ohne_wiederanbindung() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(
            quelle()
                .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
                .laufende_dauerhaft(Err(Fehler::DaemonWeg)),
        );
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
        assert_eq!(q.aufrufe().laufende_transaktionen, 3);
        assert_eq!(q.aufrufe().ausfuehren, 1);
    })
}

#[test]
fn t31_daemon_weg_zweimal_nur_ein_versuch() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren_dauerhaft(Fehler::DaemonWeg));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
        assert_eq!(q.aufrufe().ausfuehren, 2);
    })
}

#[test]
fn t32_daemon_weg_wartet_laufende_transaktion_ab() {
    lokal!(async {
        let m = monitor();
        let pfad = "/1_abcdef".to_string();
        let q = Rc::new(
            quelle()
                .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
                .mit_fehlerschritt(2)
                .wirkung_vor_fehler()
                .laufende_antworten(vec![Ok(vec![pfad.clone()]), Ok(vec![pfad.clone()]), Ok(vec![])]),
        );
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(hat(&l, |z| matches!(z, Zustand::Laeuft { status, .. } if status.contains("laufende Transaktion"))));
        assert_eq!(q.aufrufe().laufende_transaktionen, 3);
        assert_eq!(q.aufrufe().ausfuehren, 1);
    })
}

#[test]
fn t33_meldekanal_ohne_ende_gilt_als_daemon_weg() {
    lokal!(async {
        let m = monitor();
        // Skript ohne Endmeldung: Kanal schließt sich → DaemonWeg → Wiederanbindung.
        // Der zweite Lauf nutzt dasselbe Skript, endet also wieder ohne Meldung.
        let q = Rc::new(quelle().mit_skript(vec![Fortschritt::Prozent(10)]));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
        assert_eq!(q.aufrufe().ausfuehren, 2);
    })
}

#[test]
fn t34_abbruch_waehrend_wiederanbindung() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(
            quelle()
                .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
                .laufende_dauerhaft(Ok(vec!["/fremd".into()])),
        );
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, |z| matches!(z, Zustand::Laeuft { status, .. } if status.contains("laufende Transaktion"))).await;
        g.abbrechen();
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
        assert_eq!(q.aufrufe().ausfuehren, 1);
    })
}

#[test]
fn t35_wieder_anbinden_nach_app_neustart() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().laufende_antworten(vec![Ok(vec!["/7_aa".into()]), Ok(vec!["/7_aa".into()]), Ok(vec![])]));
        let (l, f) = melder();
        let griffe = m.wieder_anbinden(q.clone() as Rc<dyn Paketquelle>, f);
        assert_eq!(griffe.len(), 1);
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(hat(&l, ist_laeuft));

        let q2 = Rc::new(quelle());
        let (l2, f2) = melder();
        let _ = m.wieder_anbinden(q2.clone() as Rc<dyn Paketquelle>, f2);
        assert_eq!(warte_ende(&l2).await, Zustand::Fertig);
        assert_eq!(l2.borrow().len(), 1, "nichts lief: sofort Fertig");

        let q3 = Rc::new(quelle().laufende_dauerhaft(Err(Fehler::DaemonWeg)));
        let (l3, f3) = melder();
        let _ = m.wieder_anbinden(q3.clone() as Rc<dyn Paketquelle>, f3);
        assert_eq!(warte_ende(&l3).await, Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
    })
}

// ---------------------------------------------------------------------------
// Warteschlange
// ---------------------------------------------------------------------------

#[test]
fn t36_warteschlange_mit_drei_auftraegen() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().mit_schritten(3, Duration::from_secs(2)));
        // Gemeinsames Ereignisprotokoll aller drei, um Überlappung zu erkennen.
        let ereignisse: Rc<RefCell<Vec<(u64, Zustand)>>> = Rc::new(RefCell::new(Vec::new()));
        let mut griffe = Vec::new();
        let mut listen = Vec::new();
        for name in ["vlc", "inkscape", "gimp"] {
            let (l, f) = melder();
            let e = ereignisse.clone();
            let id_zelle = Rc::new(std::cell::Cell::new(0u64));
            let id2 = id_zelle.clone();
            let g = m.starten(
                auftrag(Vorgang::Installieren, name),
                q.clone() as Rc<dyn Paketquelle>,
                Box::new(move |z| {
                    e.borrow_mut().push((id2.get(), z.clone()));
                    f(z);
                }),
            );
            id_zelle.set(g.id);
            griffe.push(g);
            listen.push(l);
        }
        // Griffe eindeutig und aufsteigend.
        assert!(griffe[0].id < griffe[1].id && griffe[1].id < griffe[2].id);
        for l in &listen {
            assert_eq!(warte_ende(l).await, Zustand::Fertig);
        }
        // Reihenfolge erhalten.
        let namen: Vec<String> = q.ausgefuehrt().iter().map(|(_, ids)| ids[0].split(';').next().unwrap().to_string()).collect();
        assert_eq!(namen, ["vlc", "inkscape", "gimp"]);
        // Nie zwei gleichzeitig: zwischen erstem Laeuft und Fertig eines Griffs
        // kommt kein Laeuft eines anderen.
        let ev = ereignisse.borrow();
        let mut aktiv: Option<u64> = None;
        for (id, z) in ev.iter() {
            match z {
                Zustand::Laeuft { .. } => {
                    assert!(aktiv.is_none() || aktiv == Some(*id), "Überlappung: {ev:?}");
                    aktiv = Some(*id);
                }
                z if ist_ende(z) => {
                    if aktiv == Some(*id) {
                        aktiv = None;
                    }
                }
                _ => {}
            }
        }
    })
}

#[test]
fn t37_warteschlange_fehler_blockiert_nachfolger_nicht() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().fehler_bei_ausfuehren(vec![Fehler::NetzFehlt]));
        let (_a, la) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        let (_b, lb) = starte(&m, &q, auftrag(Vorgang::Installieren, "inkscape"));
        assert_eq!(warte_ende(&la).await, Zustand::Fehlgeschlagen(Fehler::NetzFehlt));
        assert_eq!(warte_ende(&lb).await, Zustand::Fertig);
    })
}

#[test]
fn t38_warteschlange_reihenfolge_bleibt_bei_offener_bestaetigung() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle());
        let (ga, la) = starte(&m, &q, auftrag(Vorgang::Entfernen, "htop"));
        let (_gb, lb) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&la, ist_bestaetigung).await;
        // A wartet auf den Benutzer. Reihenfolge bleibt erhalten: B wartet hinter A,
        // erst nach Bestätigung von A laufen beide nacheinander.
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(q.aufrufe().ausfuehren, 0);
        ga.bestaetigen();
        assert_eq!(warte_ende(&la).await, Zustand::Fertig);
        assert_eq!(warte_ende(&lb).await, Zustand::Fertig);
        let a = q.ausgefuehrt();
        assert_eq!(a[0].0, Vorgang::Entfernen);
        assert_eq!(a[1].0, Vorgang::Installieren);
    })
}

// ---------------------------------------------------------------------------
// Zeitbegrenzung
// ---------------------------------------------------------------------------

#[test]
fn t39_zeitbegrenzung_fragt_nach_statt_abzubrechen() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(quelle().haengend());
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        warte_bis(&l, ist_laeuft).await;
        let t0 = Instant::now();
        warte_bis(&l, ist_nachfrage).await;
        let d = t0.elapsed();
        assert!(d >= Duration::from_secs(600) && d < Duration::from_secs(602), "{d:?}");
        // Weiter warten: es passiert kein Abbruch, die Nachfrage kommt nach weiteren 10 min wieder.
        tokio::time::sleep(Duration::from_secs(900)).await;
        assert_eq!(q.aufrufe().abbrechen, 0);
        assert_eq!(l.borrow().iter().filter(|z| ist_nachfrage(z)).count(), 2);
        assert!(!ist_ende(l.borrow().last().unwrap()));
        g.abbrechen();
        assert_eq!(warte_ende(&l).await, Zustand::Abgebrochen);
    })
}

#[test]
fn t40_zeitbegrenzung_konfigurierbar_und_fortschritt_setzt_zurueck() {
    lokal!(async {
        let m = monitor_mit(Einstellungen { zeit_ohne_fortschritt: Duration::from_secs(60), ..Default::default() });
        // 5 Schritte à 50 s: jeweils unter der Grenze → keine Nachfrage.
        let q = Rc::new(quelle().mit_schritten(5, Duration::from_secs(50)));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(!hat(&l, ist_nachfrage));
        // Ein Schritt mit 90 s: eine Nachfrage, dann trotzdem Fertig.
        let q2 = Rc::new(quelle().mit_schritten(1, Duration::from_secs(90)));
        let (_g2, l2) = starte(&m, &q2, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l2).await, Zustand::Fertig);
        assert_eq!(l2.borrow().iter().filter(|z| ist_nachfrage(z)).count(), 1);
        assert!(l2.borrow().iter().any(|z| matches!(z, Zustand::Laeuft { status, .. } if status.contains("60 Sekunden") || status.contains("1 Minuten"))));
    })
}

// ---------------------------------------------------------------------------
// Protokoll
// ---------------------------------------------------------------------------

#[test]
fn t41_protokoll_griff_zustandswechsel_fehlercode_und_kuerzung() {
    lokal!(async {
        let protokoll = Rc::new(Protokoll::stumm());
        let m = Monitor::mit(laufzeit(), Einstellungen::default(), protokoll.clone());
        let viele: Vec<Paket> = (0..30).map(|i| paket(&format!("p{i:02}"), "1", "", false)).collect();
        let q = Rc::new(
            AttrappeQuelle::neu(laufzeit())
                .mit_paketen(viele.clone())
                .fehler_bei_ausfuehren(vec![sonstiges("repo-not-available")]),
        );
        let (l, f) = melder();
        let g = m.starten(
            Auftrag { vorgang: Vorgang::Installieren, pakete: viele, anwendung: None },
            q.clone() as Rc<dyn Paketquelle>,
            f,
        );
        assert_eq!(warte_ende(&l).await, Zustand::Fehlgeschlagen(sonstiges("repo-not-available")));
        let zeilen = protokoll.zeilen();
        let marke = format!("[#{}]", g.id);
        assert!(zeilen.iter().all(|z| z.contains(&marke)), "{zeilen:#?}");
        assert!(zeilen.iter().any(|z| z.contains("Start → Simuliert(+0 −0 ↑0")), "{zeilen:#?}");
        assert!(zeilen.iter().any(|z| z.contains("→ Fehlgeschlagen(repo-not-available")), "{zeilen:#?}");
        let auftragszeile = zeilen.iter().find(|z| z.contains("Auftrag Installieren")).unwrap();
        assert!(auftragszeile.contains("und 10 weitere"), "{auftragszeile}");
        assert!(!auftragszeile.contains("p25"));
        // Zeitstempel JJJJ-MM-TT HH:MM:SS am Zeilenanfang.
        let z0 = &zeilen[0];
        assert_eq!(&z0[4..5], "-");
        assert_eq!(&z0[13..14], ":");
    })
}

#[test]
fn t42_daemon_weg_beim_entfernen_ziel_schon_erreicht() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(
            quelle()
                .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
                .mit_fehlerschritt(4)
                .wirkung_vor_fehler(),
        );
        let (g, l) = starte(&m, &q, auftrag(Vorgang::Entfernen, "htop"));
        warte_bis(&l, ist_bestaetigung).await;
        g.bestaetigen();
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert_eq!(q.aufrufe().ausfuehren, 1);
        assert!(!q.ist_installiert("htop"));
    })
}

#[test]
fn t43_daemon_weg_beim_aktualisieren_prueft_aktualisierungsliste() {
    lokal!(async {
        let m = monitor();
        let q = Rc::new(
            AttrappeQuelle::beispiel(laufzeit())
                .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
                .mit_fehlerschritt(1),
        );
        let htop = q.katalog().into_iter().find(|p| p.name == "htop").unwrap();
        let (_g, l) = starte(&m, &q, Auftrag { vorgang: Vorgang::Aktualisieren, pakete: vec![htop], anwendung: None });
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert_eq!(q.aufrufe().aktualisierungen, 1, "Zielzustand über GetUpdates geprüft");
        assert_eq!(q.aufrufe().ausfuehren, 2, "htop war noch offen → erneut ausgeführt");
    })
}

#[test]
fn t44_protokoll_schreibt_keine_prozentschritte() {
    lokal!(async {
        let protokoll = Rc::new(Protokoll::stumm());
        let m = Monitor::mit(laufzeit(), Einstellungen::default(), protokoll.clone());
        let q = Rc::new(quelle().mit_schritten(20, Duration::from_millis(10)));
        let (_g, l) = starte(&m, &q, auftrag(Vorgang::Installieren, "vlc"));
        assert_eq!(warte_ende(&l).await, Zustand::Fertig);
        assert!(l.borrow().len() > 40, "Oberfläche bekommt jeden Schritt");
        assert!(protokoll.zeilen().len() <= 10, "{:#?}", protokoll.zeilen());
    })
}
