//! Protokoll nach `~/.local/state/querencia/software.log`.
//!
//! Format je Zeile: `JJJJ-MM-TT HH:MM:SS [#griff] Nachricht`. Zustandswechsel
//! erscheinen als `von → nach`, Fehler mit PackageKit-Fehlercode. Paketlisten
//! werden auf 20 Einträge gekürzt. Rotation: überschreitet die Datei 1 MB, wird
//! sie zu `software.log.1` umbenannt (eine Vorgängerdatei wird behalten).

use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::fehlertexte;
use crate::monitor::Zustand;
use crate::typen::{Fehler, Simulation, Vorgang};

/// Rotationsgrenze: 1 MB.
pub const ROTATIONSGRENZE_BYTES: u64 = 1_000_000;
/// Höchstens so viele Paketnamen je Protokollzeile.
pub const HOECHSTENS_PAKETE: usize = 20;

#[derive(Debug)]
pub struct Protokoll {
    pfad: Option<PathBuf>,
    grenze: u64,
    /// Letzte Zeilen im Speicher (für Tests und `qs-cli`), höchstens 200.
    spiegel: RefCell<Vec<String>>,
}

impl Protokoll {
    /// Standardpfad: `$XDG_STATE_HOME/querencia/software.log`, sonst
    /// `~/.local/state/querencia/software.log`. Lässt sich mit `QS_LOG` übersteuern
    /// (`QS_LOG=` leer = kein Protokoll).
    pub fn standard() -> Self {
        if let Ok(p) = std::env::var("QS_LOG") {
            return if p.is_empty() {
                Self::stumm()
            } else {
                Self::mit_pfad(PathBuf::from(p))
            };
        }
        Self::mit_pfad(standardpfad())
    }

    pub fn mit_pfad(pfad: PathBuf) -> Self {
        Self {
            pfad: Some(pfad),
            grenze: ROTATIONSGRENZE_BYTES,
            spiegel: RefCell::new(Vec::new()),
        }
    }

    /// Schreibt nichts auf die Platte, behält aber den Spiegel im Speicher.
    pub fn stumm() -> Self {
        Self {
            pfad: None,
            grenze: ROTATIONSGRENZE_BYTES,
            spiegel: RefCell::new(Vec::new()),
        }
    }

    /// Andere Rotationsgrenze (Tests).
    pub fn mit_grenze(mut self, bytes: u64) -> Self {
        self.grenze = bytes;
        self
    }

    pub fn pfad(&self) -> Option<&Path> {
        self.pfad.as_deref()
    }

    /// Die zuletzt geschriebenen Zeilen (höchstens 200), älteste zuerst.
    pub fn zeilen(&self) -> Vec<String> {
        self.spiegel.borrow().clone()
    }

    /// Eine Zeile ohne Griff-Bezug (Start, Fehler der Einbettung).
    pub fn allgemein(&self, nachricht: &str) {
        self.zeile(&format!("{} [--] {}", zeitstempel(), nachricht));
    }

    /// Eine Zeile zum Griff `griff`.
    pub fn schreiben(&self, griff: u64, nachricht: &str) {
        self.zeile(&format!("{} [#{}] {}", zeitstempel(), griff, nachricht));
    }

    /// Zustandswechsel `von → nach`.
    pub fn zustandswechsel(&self, griff: u64, von: Option<&Zustand>, nach: &Zustand) {
        let von = von.map(zustand_kurz).unwrap_or_else(|| "Start".to_string());
        self.schreiben(griff, &format!("{} → {}", von, zustand_kurz(nach)));
    }

    /// Auftragsbeginn mit gekürzter Paketliste.
    pub fn auftrag(&self, griff: u64, vorgang: &Vorgang, pakete: &[String]) {
        self.schreiben(
            griff,
            &format!(
                "Auftrag {} {}",
                vorgang_kurz(vorgang),
                kuerze_pakete(pakete)
            ),
        );
    }

    fn zeile(&self, text: &str) {
        {
            let mut s = self.spiegel.borrow_mut();
            s.push(text.to_string());
            if s.len() > 200 {
                let ueber = s.len() - 200;
                s.drain(0..ueber);
            }
        }
        let Some(pfad) = &self.pfad else { return };
        if let Err(e) = schreibe_datei(pfad, text, self.grenze) {
            // Kein Protokoll darf die Transaktion stören; einmalig auf stderr.
            eprintln!(
                "qs_transaktion: Protokoll {} nicht schreibbar: {}",
                pfad.display(),
                e
            );
        }
    }
}

fn schreibe_datei(pfad: &Path, text: &str, grenze: u64) -> std::io::Result<()> {
    if let Some(eltern) = pfad.parent() {
        fs::create_dir_all(eltern)?;
    }
    if let Ok(meta) = fs::metadata(pfad) {
        if meta.len() >= grenze {
            let mut alt = pfad.as_os_str().to_owned();
            alt.push(".1");
            fs::rename(pfad, PathBuf::from(alt))?;
        }
    }
    let mut f = OpenOptions::new().create(true).append(true).open(pfad)?;
    f.write_all(text.as_bytes())?;
    f.write_all(b"\n")
}

/// `$XDG_STATE_HOME/querencia/software.log` bzw. `~/.local/state/querencia/software.log`.
pub fn standardpfad() -> PathBuf {
    let basis = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    basis.join("querencia").join("software.log")
}

fn zeitstempel() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Paketliste auf höchstens [`HOECHSTENS_PAKETE`] Einträge gekürzt; Paket-IDs
/// werden auf den Namen (Teil vor dem ersten `;`) reduziert.
pub fn kuerze_pakete(pakete: &[String]) -> String {
    let namen: Vec<&str> = pakete
        .iter()
        .map(|p| p.split(';').next().unwrap_or(p.as_str()))
        .collect();
    if namen.is_empty() {
        return "[]".to_string();
    }
    if namen.len() <= HOECHSTENS_PAKETE {
        format!("[{}]", namen.join(", "))
    } else {
        format!(
            "[{} … und {} weitere] ({} insgesamt)",
            namen[..HOECHSTENS_PAKETE].join(", "),
            namen.len() - HOECHSTENS_PAKETE,
            namen.len()
        )
    }
}

pub fn vorgang_kurz(v: &Vorgang) -> &'static str {
    match v {
        Vorgang::Installieren => "Installieren",
        Vorgang::Entfernen => "Entfernen",
        Vorgang::Aktualisieren => "Aktualisieren",
        Vorgang::QuellenAuffrischen => "QuellenAuffrischen",
    }
}

fn simulation_kurz(s: &Simulation) -> String {
    format!(
        "+{} −{} ↑{} {}",
        s.installieren.len(),
        s.entfernen.len(),
        s.aktualisieren.len(),
        groesse(s.download_bytes)
    )
}

/// Kurzform eines Zustands für das Protokoll (ohne lange Listen).
pub fn zustand_kurz(z: &Zustand) -> String {
    match z {
        Zustand::Simuliert(s) => format!("Simuliert({})", simulation_kurz(s)),
        Zustand::WartetAufBestaetigung => "WartetAufBestaetigung".to_string(),
        Zustand::Laeuft { prozent, status } => match prozent {
            Some(p) => format!("Laeuft({} %, {})", p, status),
            None => format!("Laeuft(–, {})", status),
        },
        Zustand::WartetAufSperre { .. } => "WartetAufSperre".to_string(),
        Zustand::Fertig => "Fertig".to_string(),
        Zustand::Fehlgeschlagen(f) => format!("Fehlgeschlagen({})", fehler_kurz(f)),
        Zustand::Abgebrochen => "Abgebrochen".to_string(),
    }
}

fn fehler_kurz(f: &Fehler) -> String {
    fehlertexte::kurz(f)
}

/// Lesbare Größe (MiB-Basis, wie die Oberfläche).
pub fn groesse(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.1} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kuerzt_lange_listen() {
        let viele: Vec<String> = (0..27).map(|i| format!("paket{i};1;amd64;main")).collect();
        let s = kuerze_pakete(&viele);
        assert!(s.contains("und 7 weitere"), "{s}");
        assert!(s.contains("(27 insgesamt)"), "{s}");
        assert_eq!(s.matches("paket").count(), 20);
        assert!(!s.contains(';'));
    }

    #[test]
    fn kurze_liste_bleibt_vollstaendig() {
        let s = kuerze_pakete(&["a;1;x;y".into(), "b".into()]);
        assert_eq!(s, "[a, b]");
        assert_eq!(kuerze_pakete(&[]), "[]");
    }

    #[test]
    fn rotation_bei_grenze() {
        let dir = std::env::temp_dir().join(format!(
            "qs_protokoll_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pfad = dir.join("unter").join("software.log");
        let p = Protokoll::mit_pfad(pfad.clone()).mit_grenze(200);
        for i in 0..10 {
            p.schreiben(
                1,
                &format!("Zeile {i} mit etwas Text, damit die Grenze überschritten wird"),
            );
        }
        let alt = dir.join("unter").join("software.log.1");
        assert!(alt.exists(), "Rotation fehlt");
        assert!(fs::metadata(&pfad).unwrap().len() < 200 + 120);
        assert_eq!(p.zeilen().len(), 10);
        assert!(p.zeilen()[0].contains("[#1] Zeile 0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stumm_schreibt_nur_spiegel() {
        let p = Protokoll::stumm();
        p.allgemein("hallo");
        assert_eq!(p.zeilen().len(), 1);
        assert!(p.zeilen()[0].ends_with("[--] hallo"));
        assert!(p.pfad().is_none());
    }

    #[test]
    fn groessen() {
        assert_eq!(groesse(0), "0 B");
        assert_eq!(groesse(2048), "2 KiB");
        assert_eq!(groesse(52 * 1024 * 1024), "52.0 MiB");
    }
}
