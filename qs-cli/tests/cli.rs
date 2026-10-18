//! Tests des Werkzeugs `qs-cli` gegen die eingebaute Attrappe (kein Daemon nötig).

use std::process::{Command, Output};

fn qs(args: &[&str]) -> Output {
    let dir = std::env::temp_dir().join(format!("qs-cli-test-{}", std::process::id()));
    Command::new(env!("CARGO_BIN_EXE_qs-cli"))
        .args(args)
        .env("QS_LOG", dir.join("software.log"))
        .env_remove("QS_PK_BUS")
        .output()
        .expect("qs-cli starten")
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn c01_suche_attrappe() {
    let o = qs(&["suche", "querencia"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("querencia-editor"));
}

#[test]
fn c02_echt_verweigert_schreibende_befehle() {
    for v in ["installieren", "entfernen", "aktualisieren"] {
        let o = qs(&["--echt", "fuehre-aus", v, "vlc"]);
        assert_eq!(o.status.code(), Some(3), "{}", text(&o));
        assert!(text(&o).contains("verweigert"));
    }
}

#[test]
fn c03_fuehre_aus_ohne_ziel_verweigert() {
    let o = qs(&["fuehre-aus", "installieren", "vlc"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
}

#[test]
fn c04_pruefgeruest_ohne_bus() {
    let o = qs(&["--pruefgeruest", "fuehre-aus", "installieren", "vlc"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("QS_PK_BUS"));
}

#[test]
fn c05_fuehre_aus_erfolg_mit_fortschrittszeilen() {
    let o = qs(&["fuehre-aus", "--attrappe", "installieren", "vlc"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(0), "{t}");
    assert!(t.contains("Simuliert:"), "{t}");
    assert!(t.contains("Läuft 100 %"), "{t}");
    assert!(t.lines().any(|z| z.starts_with("[#1") && z.ends_with("Fertig")), "{t}");
}

#[test]
fn c06_szenarien_rueckgabewerte() {
    for (szenario, code) in [
        ("test-fail", 1),
        ("kein-agent", 1),
        ("nicht-berechtigt", 1),
        ("nicht-gefunden", 1),
        ("netz-fehlt", 1),
        ("neustart", 0),
        ("sperre-2", 0),
    ] {
        let o = qs(&["fuehre-aus", "--attrappe", "--schnell", "--szenario", szenario, "--ja", "installieren", "vlc"]);
        assert_eq!(o.status.code(), Some(code), "{szenario}: {}", text(&o));
    }
}

#[test]
fn c07_bestaetigung_verweigert() {
    let o = qs(&["fuehre-aus", "--attrappe", "--nein", "entfernen", "htop"]);
    assert_eq!(o.status.code(), Some(4), "{}", text(&o));
    assert!(text(&o).contains("Abgebrochen"));
}

#[test]
fn c08_fehlertexte_markdown() {
    let o = qs(&["fehlertexte", "--markdown"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o.stdout).lines().count(), 11);
}

#[test]
fn c09_unbekannte_option() {
    let o = qs(&["--quatsch", "suche", "x"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn c10_neustart_wird_angezeigt() {
    let o = qs(&["fuehre-aus", "--attrappe", "--szenario", "neustart", "installieren", "vlc"]);
    assert!(text(&o).contains("Fertig – Neustart erforderlich"), "{}", text(&o));
}
