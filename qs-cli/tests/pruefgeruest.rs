//! Integrationstests der Zustandsmaschine gegen SW-1s Prüfgerüst: privater
//! D-Bus mit `qs-pk-attrappe` oder `packagekitd` mit Test-Backend.
//!
//! Alle Tests sind `#[ignore]` und brauchen `QS_PK_BUS`. Ausführen über
//! `skripte/pruefgeruest-tests.sh`, das je Backend einen eigenen Prüfbus
//! startet und die passenden Tests per Namensfilter wählt:
//!
//! | Präfix | Prüfbus |
//! |---|---|
//! | `attrappe_` | `pk-pruefbus.sh start attrappe` |
//! | `verweigert_` | `pk-pruefbus.sh start attrappe --polkit verweigern` |
//! | `daemonweg_` | je Test ein frischer Prüfbus mit Attrappe (der Test beendet den Daemon) |
//! | `testfail_` | `pk-pruefbus.sh start test_fail` (echter packagekitd) |
//! | `testsucceed_` | `pk-pruefbus.sh start test_succeed` (echter packagekitd) |
//!
//! Es wird nie ein echtes Paket angefasst: `PaketKit` verbindet sich nur mit
//! dem Bus aus `QS_PK_BUS`, und jeder Test prüft vorher den Backend-Namen.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use qs_paketkit::PaketKit;
use qs_transaktion::laufzeit::GlibLaufzeit;
use qs_transaktion::{
    ist_ende, ist_nachfrage, Auftrag, Einstellungen, Fehler, Laufzeit, Monitor, Paket, Paketquelle,
    Protokoll, TransaktionsGriff, Transaktionsmonitor, Vorgang, Zustand,
};

type Liste = Rc<RefCell<Vec<Zustand>>>;

/// Kurze Fristen, damit die Fälle mit echter Uhr in Sekunden durchlaufen.
fn einstellungen() -> Einstellungen {
    Einstellungen {
        sperre_wiederholung: Duration::from_millis(300),
        sperre_hoechstdauer: Duration::from_millis(1500),
        zeit_ohne_fortschritt: Duration::from_secs(30),
        daemon_wartezeit: Duration::from_millis(500),
        daemon_versuche: 3,
        fremde_transaktionen_hoechstdauer: Duration::from_secs(10),
        abbruch_frist: Duration::from_secs(5),
        ..Default::default()
    }
}

/// Führt `f` auf einem eigenen GLib-Kontext aus (als Thread-Standard, damit
/// SW-1s `ausfuehren` seine Meldungen hierher zustellt).
fn glib_lauf<F: std::future::Future<Output = ()>>(f: F) {
    let ctx = glib::MainContext::new();
    ctx.with_thread_default(|| ctx.block_on(f))
        .expect("GLib-Kontext");
}

/// Verbindet sich mit dem Prüfbus und prüft das Backend. `None` = Test passt
/// nicht zu diesem Prüfbus (übersprungen, mit Meldung).
async fn pruefbus(erwartet: &[&str]) -> Option<Rc<PaketKit>> {
    let bus = std::env::var("QS_PK_BUS").unwrap_or_default();
    assert!(
        !bus.trim().is_empty(),
        "QS_PK_BUS fehlt – Tests über skripte/pruefgeruest-tests.sh starten"
    );
    assert!(
        !bus.contains("/run/dbus/system_bus_socket"),
        "QS_PK_BUS zeigt auf den Systembus – verweigert"
    );
    let pk = PaketKit::neu().await.expect("Verbindung zum Prüfbus");
    let backend = pk.backend_name().await.expect("BackendName");
    assert!(
        backend.starts_with("test_") || backend == "attrappe" || backend == "dummy",
        "Backend „{backend}“ ist kein Test-Backend – verweigert"
    );
    if !erwartet.contains(&backend.as_str()) {
        eprintln!("ÜBERSPRUNGEN: Backend {backend}, erwartet {erwartet:?}");
        return None;
    }
    Some(Rc::new(pk))
}

fn melder() -> (Liste, Box<dyn Fn(Zustand)>) {
    let l: Liste = Rc::new(RefCell::new(Vec::new()));
    let l2 = l.clone();
    (l, Box::new(move |z| l2.borrow_mut().push(z)))
}

fn monitor() -> Monitor {
    let laufzeit: Rc<dyn Laufzeit> = Rc::new(GlibLaufzeit);
    Monitor::mit(laufzeit, einstellungen(), Rc::new(Protokoll::standard()))
}

async fn paket(pk: &PaketKit, name: &str) -> Paket {
    pk.paket_nach_name(name)
        .await
        .expect("Resolve")
        .unwrap_or_else(|| panic!("Paket {name} fehlt im Prüfbestand"))
}

async fn starte(
    m: &Monitor,
    pk: &Rc<PaketKit>,
    v: Vorgang,
    name: &str,
) -> (TransaktionsGriff, Liste) {
    let p = paket(pk, name).await;
    let (l, f) = melder();
    let g = m.starten(
        Auftrag {
            vorgang: v,
            pakete: vec![p],
            anwendung: None,
        },
        pk.clone() as Rc<dyn Paketquelle>,
        f,
    );
    (g, l)
}

async fn warte_bis(l: &Liste, frist: Duration, pred: impl Fn(&Zustand) -> bool) {
    let ende = Instant::now() + frist;
    loop {
        if l.borrow().last().map(&pred).unwrap_or(false) {
            return;
        }
        assert!(
            Instant::now() < ende,
            "Zeitüberschreitung; Zustände: {:#?}",
            l.borrow()
        );
        glib::timeout_future(Duration::from_millis(10)).await;
    }
}

/// Wartet aufs Ende; bestätigt dabei automatisch, wenn `bestaetigen`.
async fn ende(l: &Liste, g: &TransaktionsGriff, bestaetigen: bool) -> Zustand {
    let frist = Instant::now() + Duration::from_secs(60);
    let mut bestaetigt = false;
    loop {
        let letzter = l.borrow().last().cloned();
        match letzter {
            Some(z) if ist_ende(&z) => {
                eprintln!("Zustände: {:?}", l.borrow());
                return z;
            }
            Some(Zustand::WartetAufBestaetigung) if bestaetigen && !bestaetigt => {
                g.bestaetigen();
                bestaetigt = true;
            }
            _ => {}
        }
        assert!(
            Instant::now() < frist,
            "kein Ende nach 60 s: {:#?}",
            l.borrow()
        );
        glib::timeout_future(Duration::from_millis(10)).await;
    }
}

fn hat(l: &Liste, pred: impl Fn(&Zustand) -> bool) -> bool {
    l.borrow().iter().any(pred)
}

// ---------------------------------------------------------------------------
// Attrappe von SW-1 (polkit erlaubt)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p01_installieren_erfolg() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "gedit").await;
        assert_eq!(ende(&l, &g, true).await, Zustand::Fertig);
        assert!(matches!(l.borrow()[0], Zustand::Simuliert(_)));
        assert!(hat(&l, |z| matches!(z, Zustand::Laeuft { .. })));
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p02_entfernen_mit_bestaetigung() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Entfernen, "powertop").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(z, Zustand::WartetAufBestaetigung)
        })
        .await;
        assert_eq!(ende(&l, &g, true).await, Zustand::Fertig);
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p03_bestaetigung_verweigert() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Entfernen, "powertop").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(z, Zustand::WartetAufBestaetigung)
        })
        .await;
        g.abbrechen();
        assert_eq!(ende(&l, &g, false).await, Zustand::Abgebrochen);
        assert!(!hat(&l, |z| matches!(z, Zustand::Laeuft { .. })));
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p04_abhaengigkeiten_kaputt() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "kaputt").await;
        let z = ende(&l, &g, true).await;
        assert!(
            matches!(
                z,
                Zustand::Fehlgeschlagen(Fehler::AbhaengigkeitenKonflikt(_))
            ),
            "{z:?}"
        );
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p05_sperre_dauerhaft() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let t0 = Instant::now();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "gesperrt").await;
        let z = ende(&l, &g, true).await;
        assert!(
            matches!(z, Zustand::Fehlgeschlagen(Fehler::SperreBelegt(_))),
            "{z:?}"
        );
        assert!(hat(&l, |z| matches!(z, Zustand::WartetAufSperre { .. })));
        assert!(
            t0.elapsed() >= Duration::from_millis(1500),
            "{:?}",
            t0.elapsed()
        );
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p06_abbruch_waehrend_sperre() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "gesperrt").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(z, Zustand::WartetAufSperre { .. })
        })
        .await;
        g.abbrechen();
        assert_eq!(ende(&l, &g, false).await, Zustand::Abgebrochen);
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p07_sperre_erneut_versuchen() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "gesperrt").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(z, Zustand::WartetAufSperre { .. })
        })
        .await;
        let t0 = Instant::now();
        g.sperre_erneut_versuchen();
        let z = ende(&l, &g, false).await;
        // Erneut versuchen setzt die Frist zurück: Ende frühestens nach weiteren 1,5 s.
        assert!(
            matches!(z, Zustand::Fehlgeschlagen(Fehler::SperreBelegt(_))),
            "{z:?}"
        );
        assert!(
            t0.elapsed() >= Duration::from_millis(1200),
            "{:?}",
            t0.elapsed()
        );
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p08_neustart_merkmal() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "neustart").await;
        assert_eq!(ende(&l, &g, true).await, Zustand::Fertig);
        assert!(m.braucht_neustart(g.id));
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p09_abbruch_in_laeuft() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "langsam").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(z, Zustand::Laeuft { .. })
        })
        .await;
        glib::timeout_future(Duration::from_millis(500)).await;
        g.abbrechen();
        assert_eq!(ende(&l, &g, false).await, Zustand::Abgebrochen);
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p10_zeitbegrenzung_nachfrage() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let laufzeit: Rc<dyn Laufzeit> = Rc::new(GlibLaufzeit);
        let m = Monitor::mit(
            laufzeit,
            Einstellungen {
                zeit_ohne_fortschritt: Duration::from_millis(400),
                ..einstellungen()
            },
            Rc::new(Protokoll::standard()),
        );
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "langsam").await;
        assert_eq!(
            ende(&l, &g, true).await,
            Zustand::Fertig,
            "Nachfrage bricht nicht ab"
        );
        // Ob eine Nachfrage kam, hängt davon ab, wie oft die Attrappe Fortschritt meldet.
        eprintln!(
            "Nachfragen: {}",
            l.borrow().iter().filter(|z| ist_nachfrage(z)).count()
        );
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p11_warteschlange_drei_auftraege() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let ereignisse: Rc<RefCell<Vec<(u64, Zustand)>>> = Rc::new(RefCell::new(Vec::new()));
        let mut griffe = Vec::new();
        for name in ["langsam", "querencia-editor", "neustart"] {
            let p = paket(&pk, name).await;
            let e = ereignisse.clone();
            let zelle = Rc::new(std::cell::Cell::new(0u64));
            let z2 = zelle.clone();
            let (l, f) = melder();
            let g = m.starten(
                Auftrag {
                    vorgang: Vorgang::Installieren,
                    pakete: vec![p],
                    anwendung: None,
                },
                pk.clone() as Rc<dyn Paketquelle>,
                Box::new(move |z| {
                    e.borrow_mut().push((z2.get(), z.clone()));
                    f(z);
                }),
            );
            zelle.set(g.id);
            griffe.push((g, l));
        }
        assert!(griffe[0].0.id < griffe[1].0.id && griffe[1].0.id < griffe[2].0.id);
        for (g, l) in &griffe {
            assert_eq!(ende(l, g, true).await, Zustand::Fertig);
        }
        // Reihenfolge der Ausführung = Startreihenfolge, keine Überlappung.
        let ev = ereignisse.borrow();
        let mut reihenfolge = Vec::new();
        let mut aktiv: Option<u64> = None;
        for (id, z) in ev.iter() {
            match z {
                Zustand::Laeuft { .. } => {
                    assert!(aktiv.is_none() || aktiv == Some(*id), "Überlappung: {ev:?}");
                    if aktiv.is_none() {
                        reihenfolge.push(*id);
                    }
                    aktiv = Some(*id);
                }
                z if ist_ende(z) && aktiv == Some(*id) => aktiv = None,
                _ => {}
            }
        }
        let erwartet: Vec<u64> = griffe.iter().map(|(g, _)| g.id).collect();
        assert_eq!(reihenfolge, erwartet);
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p12_wieder_anbinden_ohne_laufende() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (l, f) = melder();
        let g = m
            .wieder_anbinden(pk.clone() as Rc<dyn Paketquelle>, f)
            .remove(0);
        assert_eq!(ende(&l, &g, false).await, Zustand::Fertig);
    })
}

#[test]
#[ignore = "braucht Prüfgerüst"]
fn attrappe_p13_paket_nicht_gefunden() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (l, f) = melder();
        let fremd = Paket {
            id: "gibtsnicht;1.0;amd64;attrappe".into(),
            name: "gibtsnicht".into(),
            version: "1.0".into(),
            arch: "amd64".into(),
            zusammenfassung: String::new(),
            installiert: false,
        };
        let g = m.starten(
            Auftrag {
                vorgang: Vorgang::Installieren,
                pakete: vec![fremd],
                anwendung: None,
            },
            pk.clone() as Rc<dyn Paketquelle>,
            f,
        );
        let z = ende(&l, &g, true).await;
        assert!(matches!(z, Zustand::Fehlgeschlagen(_)), "{z:?}");
    })
}

// ---------------------------------------------------------------------------
// polkit verweigert
// ---------------------------------------------------------------------------

#[test]
#[ignore = "braucht Prüfgerüst mit --polkit verweigern"]
fn verweigert_p14_nicht_berechtigt_sofort() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let t0 = Instant::now();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "gedit").await;
        let z = ende(&l, &g, true).await;
        // SW-1 meldet KeinAgent, wenn in der Sitzung kein polkit-Agent läuft, sonst NichtBerechtigt.
        assert!(
            matches!(
                z,
                Zustand::Fehlgeschlagen(Fehler::NichtBerechtigt | Fehler::KeinAgent)
            ),
            "{z:?}"
        );
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "sofort, keine Wiederholung"
        );
        assert!(!hat(&l, |z| matches!(z, Zustand::WartetAufSperre { .. })));
    })
}

// ---------------------------------------------------------------------------
// DaemonWeg (der Test beendet den Daemon; je Test ein frischer Prüfbus)
// ---------------------------------------------------------------------------

fn daemon_pid() -> String {
    let dir = std::env::var("QS_PK_DIR").expect("QS_PK_DIR");
    std::fs::read_to_string(format!("{dir}/daemon.pid"))
        .expect("daemon.pid")
        .trim()
        .to_string()
}

fn daemon_beenden() {
    let pid = daemon_pid();
    let ok = std::process::Command::new("kill")
        .arg(&pid)
        .status()
        .expect("kill");
    assert!(ok.success(), "kill {pid}");
}

/// Startet die Attrappe neu auf demselben Bus. Der Prozess wird absichtlich nicht
/// abgewartet: `pk-pruefbus.sh stop` beendet ihn über `daemon.pid`.
#[allow(clippy::zombie_processes)]
fn daemon_neu_starten() {
    let dir = std::env::var("QS_PK_DIR").expect("QS_PK_DIR");
    let attrappe = std::env::var("QS_PK_ATTRAPPE").expect("QS_PK_ATTRAPPE");
    let bus = std::env::var("QS_PK_BUS").expect("QS_PK_BUS");
    let log = std::fs::File::create(format!("{dir}/daemon2.log")).expect("Log");
    let kind = std::process::Command::new(attrappe)
        .args(["paketkit", "--adresse", &bus])
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("Attrappe neu starten");
    std::fs::write(format!("{dir}/daemon.pid"), kind.id().to_string()).unwrap();
}

#[test]
#[ignore = "braucht frischen Prüfbus; beendet den Daemon"]
fn daemonweg_p15_ohne_wiederanbindung() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "langsam").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(
                z,
                Zustand::Laeuft {
                    prozent: Some(_),
                    ..
                }
            )
        })
        .await;
        daemon_beenden();
        let z = ende(&l, &g, true).await;
        assert_eq!(z, Zustand::Fehlgeschlagen(Fehler::DaemonWeg));
        assert!(hat(
            &l,
            |z| matches!(z, Zustand::Laeuft { status, .. } if status.contains("Wiederanbindung"))
        ));
    })
}

#[test]
#[ignore = "braucht frischen Prüfbus; beendet und startet den Daemon"]
fn daemonweg_p16_mit_wiederanbindung() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["attrappe"]).await else {
            return;
        };
        let m = monitor();
        let (g, l) = starte(&m, &pk, Vorgang::Installieren, "langsam").await;
        warte_bis(&l, Duration::from_secs(10), |z| {
            matches!(
                z,
                Zustand::Laeuft {
                    prozent: Some(_),
                    ..
                }
            )
        })
        .await;
        daemon_beenden();
        daemon_neu_starten();
        let z = ende(&l, &g, true).await;
        assert_eq!(z, Zustand::Fertig, "nach Wiederanbindung erneut ausgeführt");
        assert!(hat(
            &l,
            |z| matches!(z, Zustand::Laeuft { status, .. } if status.contains("Wiederanbindung"))
        ));
    })
}

// ---------------------------------------------------------------------------
// Echte packagekitd-Test-Backends
// ---------------------------------------------------------------------------

/// Feste Paket-ID aus den Testlisten von PackageKit (test_* kennen „powertop“).
fn testpaket() -> Paket {
    Paket {
        id: "powertop;1.8-1.fc8;i386;fedora".into(),
        name: "powertop".into(),
        version: "1.8-1.fc8".into(),
        arch: "i386".into(),
        zusammenfassung: String::new(),
        installiert: false,
    }
}

/// Wartet höchstens `frist` auf `f`; `None` = Zeitüberschreitung.
async fn mit_frist<T>(frist: Duration, f: impl std::future::Future<Output = T>) -> Option<T> {
    use futures::FutureExt;
    let mut f = Box::pin(f).fuse();
    let mut t = Box::pin(glib::timeout_future(frist)).fuse();
    futures::select! {
        v = f => Some(v),
        _ = t => None,
    }
}

async fn testbackend_lauf(
    pk: &Rc<PaketKit>,
    vorgang: Vorgang,
    pakete: Vec<Paket>,
) -> (Liste, Zustand) {
    let m = monitor();
    let (l, f) = melder();
    let g = m.starten(
        Auftrag {
            vorgang,
            pakete,
            anwendung: None,
        },
        pk.clone() as Rc<dyn Paketquelle>,
        f,
    );
    let z = ende(&l, &g, true).await;
    (l, z)
}

#[test]
#[ignore = "braucht Prüfgerüst mit test_fail"]
fn testfail_p17_fehler_aus_test_fail() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["test_fail"]).await else {
            return;
        };
        // test_fail lässt die Suche scheitern („internal-error: Error number 1“) …
        let r = mit_frist(Duration::from_secs(10), pk.suchen("power", false)).await;
        let r = r.expect("Suche gegen test_fail endet");
        assert!(
            matches!(&r, Err(Fehler::Sonstiges { code, .. }) if code == "internal-error"),
            "{r:?}"
        );
        // … schreibende Vorgänge dagegen beendet PackageKit 1.3.4 mit Erfolg
        // (Finished exit=success, kein ErrorCode; per busctl monitor belegt).
        // Der Monitor muss in jedem Fall ein definiertes Ende melden.
        for v in [
            Vorgang::Installieren,
            Vorgang::Entfernen,
            Vorgang::Aktualisieren,
        ] {
            let (_l, z) = testbackend_lauf(&pk, v.clone(), vec![testpaket()]).await;
            assert_eq!(z, Zustand::Fertig, "{v:?}");
        }
        let (_l, z) = testbackend_lauf(&pk, Vorgang::QuellenAuffrischen, vec![]).await;
        assert_eq!(z, Zustand::Fertig);
    })
}

#[test]
#[ignore = "braucht Prüfgerüst mit test_succeed"]
fn testsucceed_p18_erfolg_ueber_echten_packagekitd() {
    glib_lauf(async {
        let Some(pk) = pruefbus(&["test_succeed"]).await else {
            return;
        };
        for v in [
            Vorgang::Installieren,
            Vorgang::Entfernen,
            Vorgang::Aktualisieren,
        ] {
            let (l, z) = testbackend_lauf(&pk, v.clone(), vec![testpaket()]).await;
            assert_eq!(z, Zustand::Fertig, "{v:?}: {:?}", l.borrow());
            assert!(matches!(l.borrow()[0], Zustand::Simuliert(_)));
            if v == Vorgang::Entfernen {
                assert!(hat(&l, |z| matches!(z, Zustand::WartetAufBestaetigung)));
            }
        }
        // Befund (an SW-1 gemeldet): die Suche bekommt bei test_succeed nie `Finished`
        // (packagekitd: „pk_backend_job_finished: assertion failed“). Nur protokollieren.
        match mit_frist(Duration::from_secs(5), pk.suchen("power", false)).await {
            None => eprintln!("BEFUND: Suche gegen test_succeed nach 5 s ohne Antwort"),
            Some(r) => eprintln!("Suche gegen test_succeed: {r:?}"),
        }
    })
}
