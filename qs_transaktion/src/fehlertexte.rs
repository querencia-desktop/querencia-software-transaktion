//! Deutsche Fehlertexte, zentral. Jede [`Fehler`]-Variante bekommt eine
//! Überschrift, eine Erklärung und eine empfohlene Handlung (für die Oberfläche)
//! sowie den PackageKit-Fehlercode (für das Protokoll).

use crate::typen::Fehler;

/// Text für die Oberfläche: Überschrift (Dialogtitel), Erklärung (ein bis drei
/// Sätze) und empfohlene Handlung (ein Satz, Imperativ).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fehlertext {
    pub ueberschrift: String,
    pub erklaerung: String,
    pub handlung: String,
}

/// Liefert den Fehlertext zu einem Fehler.
pub fn fehlertext(fehler: &Fehler) -> Fehlertext {
    let (ueberschrift, erklaerung, handlung) = match fehler {
        Fehler::NichtBerechtigt => (
            "Keine Berechtigung".to_string(),
            "Die Berechtigung zum Verwalten von Software wurde nicht erteilt. Entweder wurde \
             der Berechtigungsdialog abgebrochen, das Kennwort war falsch oder die Richtlinie \
             (polkit) erlaubt diese Aktion für Ihr Konto nicht."
                .to_string(),
            "Versuchen Sie es erneut und bestätigen Sie den Berechtigungsdialog, oder wenden \
             Sie sich an die Systemverwaltung."
                .to_string(),
        ),
        Fehler::KeinAgent => (
            "Berechtigungsdialog fehlt".to_string(),
            "Es fehlt der Berechtigungsdialog der Sitzung (polkit-Agent). Ohne ihn kann kein \
             Programm Pakete installieren oder entfernen. Querencia Sitzung (G2-K) liefert ihn \
             als „querencia-polkit-agent“."
                .to_string(),
            "Starten Sie den Berechtigungsdialog aus einem Terminal der Sitzung \
             („querencia-polkit-agent“) oder melden Sie sich neu an, nachdem er eingerichtet \
             wurde, und wiederholen Sie den Vorgang."
                .to_string(),
        ),
        Fehler::SperreBelegt(wer) => (
            "Paketverwaltung ist belegt".to_string(),
            if wer.trim().is_empty() {
                "Ein anderes Programm verwendet gerade die Paketverwaltung, zum Beispiel \
                 automatische Aktualisierungen (unattended-upgrades) oder „apt“ in einem Terminal."
                    .to_string()
            } else {
                format!(
                    "Ein anderes Programm verwendet gerade die Paketverwaltung: {}. Solange \
                     es läuft, kann Querencia Software keine Pakete ändern.",
                    wer.trim()
                )
            },
            "Warten Sie, bis das andere Programm fertig ist; Querencia Software versucht es \
             alle 10 Sekunden erneut (bis zu 5 Minuten). Oder brechen Sie ab."
                .to_string(),
        ),
        Fehler::NetzFehlt => (
            "Keine Netzwerkverbindung".to_string(),
            "Die Paketquellen sind nicht erreichbar. Entweder besteht keine Verbindung zum \
             Internet, oder der Spiegelserver antwortet nicht."
                .to_string(),
            "Prüfen Sie die Netzwerkverbindung und versuchen Sie es dann erneut.".to_string(),
        ),
        Fehler::PaketNichtGefunden(name) => (
            "Paket nicht gefunden".to_string(),
            format!(
                "Das Paket „{}“ ist in den eingerichteten Paketquellen nicht vorhanden oder \
                 die Quellenliste ist veraltet.",
                name
            ),
            "Aktualisieren Sie die Paketquellen (Datei → Aktualisierungen prüfen) und suchen \
             Sie erneut."
                .to_string(),
        ),
        Fehler::AbhaengigkeitenKonflikt(text) => (
            "Abhängigkeiten nicht auflösbar".to_string(),
            format!(
                "Die Änderung widerspricht anderen installierten Paketen oder benötigt Pakete, \
                 die nicht verfügbar sind. Meldung der Paketverwaltung: {}",
                kuerze(text, 300)
            ),
            "Lassen Sie das System unverändert; prüfen Sie die Aktualisierungen oder lösen \
             Sie den Konflikt mit einem Expertenwerkzeug (apt) auf."
                .to_string(),
        ),
        Fehler::DaemonWeg => (
            "Verbindung zur Paketverwaltung verloren".to_string(),
            "Der Dienst PackageKit wurde während des Vorgangs beendet oder neu gestartet. \
             Querencia Software hat einmal versucht, den Vorgang wieder aufzunehmen; das ist \
             nicht gelungen. Die Paketdatenbank bleibt in einem konsistenten Zustand, der \
             Vorgang wurde möglicherweise nicht abgeschlossen."
                .to_string(),
            "Prüfen Sie, ob das Paket installiert ist (Ansicht „Installiert“), und wiederholen \
             Sie den Vorgang bei Bedarf."
                .to_string(),
        ),
        Fehler::Abgebrochen => (
            "Vorgang abgebrochen".to_string(),
            "Der Vorgang wurde auf Ihren Wunsch abgebrochen. Bereits heruntergeladene Dateien \
             bleiben im Zwischenspeicher, am System wurde nichts geändert."
                .to_string(),
            "Keine Handlung nötig. Starten Sie den Vorgang erneut, wenn Sie ihn doch ausführen \
             möchten."
                .to_string(),
        ),
        Fehler::Sonstiges { code, text } => (
            "Fehler der Paketverwaltung".to_string(),
            format!(
                "PackageKit meldet den Fehler „{}“. Details: {}",
                code,
                if text.trim().is_empty() {
                    "keine".to_string()
                } else {
                    kuerze(text, 300)
                }
            ),
            "Versuchen Sie es später erneut. Bleibt der Fehler, notieren Sie den Fehlercode \
             und sehen Sie im Protokoll ~/.local/state/querencia/software.log nach."
                .to_string(),
        ),
    };
    Fehlertext {
        ueberschrift,
        erklaerung,
        handlung,
    }
}

/// PackageKit-Fehlercode (`PkErrorEnum` als Text) für das Protokoll. Für
/// `Sonstiges` der gemeldete Code, für `KeinAgent` der Querencia-eigene Code
/// `kein-polkit-agent`, da PackageKit diesen Fall nur als `not-authorized` kennt.
pub fn packagekit_code(fehler: &Fehler) -> String {
    match fehler {
        Fehler::NichtBerechtigt => "not-authorized".to_string(),
        Fehler::KeinAgent => "kein-polkit-agent".to_string(),
        Fehler::SperreBelegt(_) => "cannot-get-lock".to_string(),
        Fehler::NetzFehlt => "no-network".to_string(),
        Fehler::PaketNichtGefunden(_) => "package-not-found".to_string(),
        Fehler::AbhaengigkeitenKonflikt(_) => "dep-resolution-failed".to_string(),
        Fehler::DaemonWeg => "daemon-weg".to_string(),
        Fehler::Abgebrochen => "transaction-cancelled".to_string(),
        Fehler::Sonstiges { code, .. } => {
            if code.trim().is_empty() {
                "unknown".to_string()
            } else {
                code.trim().to_string()
            }
        }
    }
}

/// Einzeiler „code: Überschrift (Details)“ für Protokoll und Kommandozeile.
pub fn kurz(fehler: &Fehler) -> String {
    let text = fehlertext(fehler);
    let details = match fehler {
        Fehler::SperreBelegt(t)
        | Fehler::PaketNichtGefunden(t)
        | Fehler::AbhaengigkeitenKonflikt(t) => t.trim().to_string(),
        Fehler::Sonstiges { text, .. } => text.trim().to_string(),
        _ => String::new(),
    };
    if details.is_empty() {
        format!("{}: {}", packagekit_code(fehler), text.ueberschrift)
    } else {
        format!(
            "{}: {} ({})",
            packagekit_code(fehler),
            text.ueberschrift,
            kuerze(&details, 120)
        )
    }
}

/// Alle Varianten mit Beispielwerten – Grundlage der Tabelle im Bericht und
/// Vollständigkeitsprüfung in den Tests.
pub fn alle_varianten() -> Vec<(&'static str, Fehler)> {
    vec![
        ("NichtBerechtigt", Fehler::NichtBerechtigt),
        ("KeinAgent", Fehler::KeinAgent),
        (
            "SperreBelegt",
            Fehler::SperreBelegt("unattended-upgrades".to_string()),
        ),
        ("NetzFehlt", Fehler::NetzFehlt),
        (
            "PaketNichtGefunden",
            Fehler::PaketNichtGefunden("beispiel".to_string()),
        ),
        (
            "AbhaengigkeitenKonflikt",
            Fehler::AbhaengigkeitenKonflikt("beispiel hängt von libfoo ab".to_string()),
        ),
        ("DaemonWeg", Fehler::DaemonWeg),
        ("Abgebrochen", Fehler::Abgebrochen),
        (
            "Sonstiges",
            Fehler::Sonstiges {
                code: "repo-not-available".to_string(),
                text: "Beispiel".to_string(),
            },
        ),
    ]
}

/// Markdown-Tabelle aller Varianten (für den Bericht; `qs-cli fehlertexte`).
pub fn tabelle_markdown() -> String {
    let mut s = String::from(
        "| Variante | PackageKit-Code | Überschrift | Erklärung | Empfohlene Handlung |\n|---|---|---|---|---|\n",
    );
    for (name, fehler) in alle_varianten() {
        let t = fehlertext(&fehler);
        s.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            name,
            packagekit_code(&fehler),
            t.ueberschrift,
            t.erklaerung,
            t.handlung
        ));
    }
    s
}

fn kuerze(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        let mut s: String = t.chars().take(max).collect();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jede_variante_hat_drei_texte() {
        for (name, fehler) in alle_varianten() {
            let t = fehlertext(&fehler);
            assert!(!t.ueberschrift.is_empty(), "{name}");
            assert!(t.erklaerung.len() > 20, "{name}");
            assert!(t.handlung.len() > 10, "{name}");
            assert!(!packagekit_code(&fehler).is_empty(), "{name}");
        }
    }

    #[test]
    fn kein_agent_nennt_polkit() {
        let t = fehlertext(&Fehler::KeinAgent);
        assert!(t.erklaerung.contains("polkit-Agent"));
        assert!(t.erklaerung.contains("G2-K"));
    }

    #[test]
    fn sonstiges_uebernimmt_code() {
        let f = Fehler::Sonstiges {
            code: " repo-not-available ".into(),
            text: String::new(),
        };
        assert_eq!(packagekit_code(&f), "repo-not-available");
        assert!(kurz(&f).starts_with("repo-not-available: "));
    }

    #[test]
    fn tabelle_hat_neun_zeilen() {
        let t = tabelle_markdown();
        assert_eq!(t.lines().count(), 2 + 9);
    }
}
