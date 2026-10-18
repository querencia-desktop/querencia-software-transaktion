//! `qs-cli` – Kommandozeilenwerkzeug für Querencia Software (SW-4).
//!
//! Zum Nachvollziehen und für die Integration: dieselbe Zustandsmaschine wie die
//! Oberfläche, mit Fortschrittszeilen im Terminal. Ziele:
//!
//! * `--attrappe` (Standard): eingebaute `AttrappeQuelle`, ändert nichts.
//! * `--pruefgeruest`: SW-1s `PaketKit` auf dem privaten Bus aus `QS_PK_BUS`;
//!   schreibende Befehle nur, wenn dort ein `test_*`- bzw. `dummy`-Backend läuft.
//! * `--echt`: SW-1s `PaketKit` auf dem Systembus, **nur lesende Befehle**.

use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt;
use qs_transaktion::attrappe::{paket, AttrappeQuelle};
use qs_transaktion::fehlertexte;
use qs_transaktion::laufzeit::GlibLaufzeit;
use qs_transaktion::protokoll::{groesse, Protokoll};
use qs_transaktion::{
    ist_ende, ist_nachfrage, Auftrag, Einstellungen, Fehler, Laufzeit, Monitor, Paket,
    Paketquelle, Simulation, TransaktionsGriff, Transaktionsmonitor, Vorgang, Zustand,
};

const EXIT_OK: i32 = 0;
const EXIT_FEHLER: i32 = 1;
const EXIT_AUFRUF: i32 = 2;
const EXIT_VERWEIGERT: i32 = 3;
const EXIT_ABGEBROCHEN: i32 = 4;

const HILFE: &str = "\
qs-cli – Querencia Software auf der Kommandozeile

Aufruf:
  qs-cli [ZIEL] [OPTIONEN] BEFEHL [ARGUMENTE]

Ziele (eins davon, Standard --attrappe):
  --attrappe        eingebaute Attrappe, ändert nichts am System
  --pruefgeruest    PackageKit auf dem privaten Bus aus QS_PK_BUS (Prüfgerüst von SW-1);
                    schreibende Befehle nur mit test_*/dummy/attrappe-Backend
  --echt            PackageKit auf dem Systembus, NUR lesende Befehle

Befehle:
  suche TEXT [--installiert]          Pakete nach Name suchen
  details PAKET                       Details (Paketname oder Paket-ID)
  updates                             verfügbare Aktualisierungen
  simuliere installieren|entfernen|aktualisieren PAKET …
                                      zeigen, was zusätzlich geschähe
  fuehre-aus installieren|entfernen|aktualisieren PAKET …
  fuehre-aus quellen-auffrischen      Transaktion mit Fortschrittszeilen
                                      (nur --attrappe oder --pruefgeruest)
  laufende                            laufende Transaktionen des Daemons beobachten
  fehlertexte [--markdown]            alle Fehlertexte der Oberfläche
  hilfe                               diese Hilfe

Optionen:
  --ja, -j            Bestätigung automatisch erteilen
  --nein              Bestätigung automatisch verweigern
  --schnell           kurze Fristen (Sperre 1 s/10 s, kein Fortschritt 5 s) zum Ausprobieren
  --abbrechen-nach S  laufende Transaktion nach S Sekunden abbrechen (Vorführung, Tests)
  --szenario NAME     nur --attrappe: erfolg (Standard), test-fail, sperre-2, sperre-dauerhaft,
                      daemon-weg, daemon-weg-dauerhaft, kein-agent, nicht-berechtigt,
                      netz-fehlt, nicht-gefunden, haengt, neustart, zusatz, gross

Strg+C beendet qs-cli sofort (eine Transaktion im Daemon läuft dann weiter;
„qs-cli laufende“ zeigt sie). Geordneter Abbruch: --abbrechen-nach.
Protokoll: ~/.local/state/querencia/software.log (übersteuerbar mit QS_LOG).
Rückgabewerte: 0 erfolgreich, 1 fehlgeschlagen, 2 Aufruffehler, 3 verweigert, 4 abgebrochen.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ziel {
    Attrappe,
    Pruefgeruest,
    Echt,
}

#[derive(Debug, Default)]
struct Optionen {
    ziel: Option<Ziel>,
    ja: bool,
    nein: bool,
    schnell: bool,
    installiert: bool,
    markdown: bool,
    szenario: Option<String>,
    abbrechen_nach: Option<f64>,
    rest: Vec<String>,
}

fn optionen_lesen(args: Vec<String>) -> Result<Optionen, String> {
    let mut o = Optionen::default();
    let mut it = args.into_iter();
    let ziel = |o: &mut Optionen, z: Ziel| -> Result<(), String> {
        if o.ziel.is_some_and(|alt| alt != z) {
            return Err("Bitte nur ein Ziel angeben (--attrappe, --pruefgeruest oder --echt).".into());
        }
        o.ziel = Some(z);
        Ok(())
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--attrappe" => ziel(&mut o, Ziel::Attrappe)?,
            "--pruefgeruest" | "--prüfgerüst" => ziel(&mut o, Ziel::Pruefgeruest)?,
            "--echt" => ziel(&mut o, Ziel::Echt)?,
            "--ja" | "-j" => o.ja = true,
            "--nein" => o.nein = true,
            "--schnell" => o.schnell = true,
            "--installiert" => o.installiert = true,
            "--markdown" => o.markdown = true,
            "--hilfe" | "-h" | "--help" => o.rest = vec!["hilfe".into()],
            "--szenario" => {
                o.szenario = Some(it.next().ok_or("--szenario braucht einen Namen")?);
            }
            "--abbrechen-nach" => {
                let v = it.next().ok_or("--abbrechen-nach braucht Sekunden")?;
                o.abbrechen_nach = Some(v.replace(',', ".").parse().map_err(|_| format!("ungültige Sekunden „{v}“"))?);
            }
            s if s.starts_with("--szenario=") => {
                o.szenario = Some(s["--szenario=".len()..].to_string());
            }
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(format!("Unbekannte Option „{s}“. Siehe „qs-cli hilfe“."));
            }
            _ => o.rest.push(a),
        }
    }
    if o.ja && o.nein {
        return Err("--ja und --nein schließen sich aus.".into());
    }
    if o.szenario.is_some() && o.ziel.unwrap_or(Ziel::Attrappe) != Ziel::Attrappe {
        return Err("--szenario gibt es nur mit --attrappe.".into());
    }
    Ok(o)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match optionen_lesen(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("qs-cli: {e}");
            std::process::exit(EXIT_AUFRUF);
        }
    };
    let ctx = glib::MainContext::default();
    let code = ctx.block_on(haupt(o));
    std::process::exit(code);
}

async fn haupt(o: Optionen) -> i32 {
    let befehl = o.rest.first().cloned().unwrap_or_else(|| "hilfe".into());
    let args: Vec<String> = o.rest.iter().skip(1).cloned().collect();
    let ziel = o.ziel.unwrap_or(Ziel::Attrappe);

    match befehl.as_str() {
        "hilfe" => {
            println!("{HILFE}");
            return EXIT_OK;
        }
        "fehlertexte" => {
            fehlertexte_zeigen(o.markdown);
            return EXIT_OK;
        }
        _ => {}
    }

    let schreibend = befehl == "fuehre-aus" || befehl == "führe-aus";
    if schreibend && ziel == Ziel::Echt {
        eprintln!(
            "qs-cli: verweigert – „{befehl}“ ändert Pakete und ist mit --echt nicht erlaubt.\n\
             Gegen den echten Daemon sind nur suche, details, updates, simuliere und laufende zulässig.\n\
             Zum Ausprobieren: --attrappe oder --pruefgeruest (privater Bus, test_*-Backend)."
        );
        return EXIT_VERWEIGERT;
    }
    if schreibend && o.ziel.is_none() {
        eprintln!(
            "qs-cli: Bitte für „fuehre-aus“ das Ziel ausdrücklich angeben: --attrappe oder --pruefgeruest."
        );
        return EXIT_AUFRUF;
    }

    let laufzeit: Rc<dyn Laufzeit> = Rc::new(GlibLaufzeit);
    let quelle = match quelle_bauen(ziel, &o, laufzeit.clone(), schreibend).await {
        Ok(q) => q,
        Err(code) => return code,
    };

    match befehl.as_str() {
        "suche" => suche(&*quelle, &args.join(" "), o.installiert).await,
        "details" => details(&*quelle, &args).await,
        "updates" | "aktualisierungen" => updates(&*quelle).await,
        "simuliere" => simuliere(&*quelle, &args).await,
        "fuehre-aus" | "führe-aus" => fuehre_aus(quelle, laufzeit, &o, &args).await,
        "laufende" => laufende(quelle, laufzeit, &o).await,
        anders => {
            eprintln!("qs-cli: unbekannter Befehl „{anders}“. Siehe „qs-cli hilfe“.");
            EXIT_AUFRUF
        }
    }
}

// ---------------------------------------------------------------------------
// Quellen
// ---------------------------------------------------------------------------

async fn quelle_bauen(
    ziel: Ziel,
    o: &Optionen,
    laufzeit: Rc<dyn Laufzeit>,
    schreibend: bool,
) -> Result<Rc<dyn Paketquelle>, i32> {
    match ziel {
        Ziel::Attrappe => {
            let q = attrappe(laufzeit, o.szenario.as_deref().unwrap_or("erfolg")).map_err(|e| {
                eprintln!("qs-cli: {e}");
                EXIT_AUFRUF
            })?;
            Ok(Rc::new(q))
        }
        Ziel::Echt | Ziel::Pruefgeruest => paketkit(ziel, schreibend).await,
    }
}

#[cfg(feature = "paketkit")]
async fn paketkit(ziel: Ziel, schreibend: bool) -> Result<Rc<dyn Paketquelle>, i32> {
    use qs_paketkit::{PaketKit, UMGEBUNG_BUS};
    let bus = std::env::var(UMGEBUNG_BUS).unwrap_or_default();
    let pk = match ziel {
        Ziel::Echt => {
            if !bus.trim().is_empty() {
                eprintln!(
                    "qs-cli: {UMGEBUNG_BUS} ist gesetzt ({bus}); --echt meint den Systembus. \
                     Für das Prüfgerüst --pruefgeruest verwenden, sonst {UMGEBUNG_BUS} entfernen."
                );
                return Err(EXIT_AUFRUF);
            }
            PaketKit::neu().await
        }
        _ => {
            if bus.trim().is_empty() {
                eprintln!(
                    "qs-cli: --pruefgeruest braucht die Adresse des privaten Busses in {UMGEBUNG_BUS} \
                     (siehe PRUEFGERUEST.md von SW-1)."
                );
                return Err(EXIT_AUFRUF);
            }
            PaketKit::neu().await
        }
    };
    let pk = match pk {
        Ok(pk) => pk,
        Err(f) => {
            fehler_zeigen(&f);
            return Err(EXIT_FEHLER);
        }
    };
    if ziel == Ziel::Pruefgeruest {
        // Sicherung: niemals echte Pakete ändern. Nur Test-Backends zulassen.
        if bus.contains("/run/dbus/system_bus_socket") {
            eprintln!("qs-cli: verweigert – {UMGEBUNG_BUS} zeigt auf den Systembus.");
            return Err(EXIT_VERWEIGERT);
        }
        let backend = pk.backend_name().await.unwrap_or_default();
        // test_* = packagekitd-Test-Backends, attrappe = qs-pk-attrappe von SW-1.
        let test = backend.starts_with("test_") || backend == "dummy" || backend == "attrappe";
        eprintln!("qs-cli: Prüfgerüst {bus}, Backend „{backend}“");
        if schreibend && !test {
            eprintln!(
                "qs-cli: verweigert – das Backend „{backend}“ ist kein Test-Backend. \
                 Schreibende Befehle laufen nur gegen test_*, dummy oder die Attrappe von SW-1."
            );
            return Err(EXIT_VERWEIGERT);
        }
    }
    Ok(Rc::new(pk))
}

#[cfg(not(feature = "paketkit"))]
async fn paketkit(_ziel: Ziel, _schreibend: bool) -> Result<Rc<dyn Paketquelle>, i32> {
    eprintln!("qs-cli: ohne Feature „paketkit“ gebaut; nur --attrappe verfügbar.");
    Err(EXIT_AUFRUF)
}

fn attrappe(laufzeit: Rc<dyn Laufzeit>, szenario: &str) -> Result<AttrappeQuelle, String> {
    let q = AttrappeQuelle::beispiel(laufzeit).mit_schritten(5, Duration::from_millis(400));
    let sperre = || Fehler::SperreBelegt("unattended-upgrades (Attrappe)".into());
    Ok(match szenario {
        "erfolg" => q,
        "test-fail" => q
            .fehler_bei_ausfuehren_dauerhaft(Fehler::Sonstiges {
                code: "internal-error".into(),
                text: "Attrappe: wie das PackageKit-Backend test_fail".into(),
            })
            .mit_fehlerschritt(2),
        "sperre-2" => q.fehler_bei_ausfuehren(vec![sperre(), sperre()]).mit_fehlerschritt(0),
        "sperre-dauerhaft" => q.fehler_bei_ausfuehren_dauerhaft(sperre()).mit_fehlerschritt(0),
        "daemon-weg" => q.fehler_bei_ausfuehren(vec![Fehler::DaemonWeg]).mit_fehlerschritt(2),
        "daemon-weg-dauerhaft" => q
            .fehler_bei_ausfuehren(vec![Fehler::DaemonWeg])
            .mit_fehlerschritt(2)
            .laufende_dauerhaft(Err(Fehler::DaemonWeg)),
        "kein-agent" => q.fehler_bei_ausfuehren_dauerhaft(Fehler::KeinAgent).mit_fehlerschritt(0),
        "nicht-berechtigt" => q
            .fehler_bei_ausfuehren_dauerhaft(Fehler::NichtBerechtigt)
            .mit_fehlerschritt(0),
        "netz-fehlt" => q.fehler_bei_ausfuehren_dauerhaft(Fehler::NetzFehlt).mit_fehlerschritt(1),
        "nicht-gefunden" => {
            q.fehler_bei_simulation_dauerhaft(Fehler::PaketNichtGefunden("(Attrappe)".into()))
        }
        "haengt" => q.haengend(),
        "neustart" => q.mit_neustart(),
        "zusatz" => q.mit_zusatz_installieren(vec![
            paket("gimp-data", "2.10.38-1", "Daten für GIMP", false),
            paket("libgimp2.0", "2.10.38-1", "Bibliotheken für GIMP", false),
        ]),
        "gross" => q.mit_download_bytes(180 * 1024 * 1024),
        anders => return Err(format!("unbekanntes Szenario „{anders}“. Siehe „qs-cli hilfe“.")),
    })
}

// ---------------------------------------------------------------------------
// Lesende Befehle
// ---------------------------------------------------------------------------

async fn suche(q: &dyn Paketquelle, text: &str, nur_installiert: bool) -> i32 {
    if text.trim().is_empty() {
        eprintln!("qs-cli: suche braucht einen Suchtext.");
        return EXIT_AUFRUF;
    }
    match q.suchen(text, nur_installiert).await {
        Ok(treffer) => {
            for p in &treffer {
                println!(
                    "{:<1} {:<32} {:<22} {}",
                    if p.installiert { "i" } else { " " },
                    p.name,
                    p.version,
                    p.zusammenfassung
                );
            }
            println!("{} Treffer", treffer.len());
            EXIT_OK
        }
        Err(f) => {
            fehler_zeigen(&f);
            EXIT_FEHLER
        }
    }
}

/// Paketname oder Paket-ID → Paket (installierte Version bevorzugt).
async fn aufloesen(q: &dyn Paketquelle, name_oder_id: &str) -> Result<Paket, Fehler> {
    if name_oder_id.contains(';') {
        let name = name_oder_id.split(';').next().unwrap_or(name_oder_id);
        if let Some(p) = q.paket_nach_name(name).await? {
            if p.id == name_oder_id {
                return Ok(p);
            }
        }
        return Ok(Paket {
            id: name_oder_id.to_string(),
            name: name.to_string(),
            version: name_oder_id.split(';').nth(1).unwrap_or("").to_string(),
            arch: name_oder_id.split(';').nth(2).unwrap_or("").to_string(),
            zusammenfassung: String::new(),
            installiert: name_oder_id.ends_with(";installed"),
        });
    }
    q.paket_nach_name(name_oder_id)
        .await?
        .ok_or_else(|| Fehler::PaketNichtGefunden(name_oder_id.to_string()))
}

async fn details(q: &dyn Paketquelle, args: &[String]) -> i32 {
    let [arg] = args else {
        eprintln!("qs-cli: details braucht genau ein Paket (Name oder Paket-ID).");
        return EXIT_AUFRUF;
    };
    let p = match aufloesen(q, arg).await {
        Ok(p) => p,
        Err(f) => {
            fehler_zeigen(&f);
            return EXIT_FEHLER;
        }
    };
    match q.details(&p.id).await {
        Ok(d) => {
            println!("Paket:        {}", d.paket.name);
            println!("Paket-ID:     {}", d.paket.id);
            println!("Version:      {}", d.paket.version);
            println!("Architektur:  {}", d.paket.arch);
            println!("Zustand:      {}", if p.installiert { "installiert" } else { "verfügbar" });
            println!("Lizenz:       {}", d.lizenz);
            println!("Größe:        {}", groesse(d.groesse_bytes));
            println!("Gruppe:       {}", d.gruppe);
            if let Some(u) = &d.url {
                println!("Homepage:     {u}");
            }
            println!("Beschreibung:");
            for z in d.beschreibung.lines() {
                println!("  {z}");
            }
            EXIT_OK
        }
        Err(f) => {
            fehler_zeigen(&f);
            EXIT_FEHLER
        }
    }
}

async fn updates(q: &dyn Paketquelle) -> i32 {
    match q.aktualisierungen().await {
        Ok(liste) => {
            for a in &liste {
                println!(
                    "{:<32} {:<22} {:<15} {}",
                    a.paket.name,
                    a.paket.version,
                    format!("{:?}", a.dringlichkeit),
                    if a.neustart_noetig { "Neustart nötig" } else { "" }
                );
            }
            println!("{} Aktualisierung(en)", liste.len());
            EXIT_OK
        }
        Err(f) => {
            fehler_zeigen(&f);
            EXIT_FEHLER
        }
    }
}

fn vorgang_lesen(s: Option<&String>) -> Result<Vorgang, String> {
    match s.map(String::as_str) {
        Some("installieren") => Ok(Vorgang::Installieren),
        Some("entfernen") => Ok(Vorgang::Entfernen),
        Some("aktualisieren") => Ok(Vorgang::Aktualisieren),
        Some("quellen-auffrischen") => Ok(Vorgang::QuellenAuffrischen),
        Some(a) => Err(format!(
            "unbekannter Vorgang „{a}“ (installieren, entfernen, aktualisieren, quellen-auffrischen)"
        )),
        None => Err("Vorgang fehlt (installieren, entfernen, aktualisieren)".into()),
    }
}

async fn pakete_aufloesen(q: &dyn Paketquelle, namen: &[String]) -> Result<Vec<Paket>, Fehler> {
    let mut v = Vec::new();
    for n in namen {
        v.push(aufloesen(q, n).await?);
    }
    Ok(v)
}

async fn simuliere(q: &dyn Paketquelle, args: &[String]) -> i32 {
    let vorgang = match vorgang_lesen(args.first()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("qs-cli: {e}");
            return EXIT_AUFRUF;
        }
    };
    if args.len() < 2 {
        eprintln!("qs-cli: simuliere braucht mindestens ein Paket.");
        return EXIT_AUFRUF;
    }
    let pakete = match pakete_aufloesen(q, &args[1..]).await {
        Ok(p) => p,
        Err(f) => {
            fehler_zeigen(&f);
            return EXIT_FEHLER;
        }
    };
    match q
        .simulieren(vorgang, pakete.iter().map(|p| p.id.clone()).collect())
        .await
    {
        Ok(sim) => {
            println!(
                "Angefragt: {}",
                pakete.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            );
            simulation_zeigen(&sim, "");
            EXIT_OK
        }
        Err(f) => {
            fehler_zeigen(&f);
            EXIT_FEHLER
        }
    }
}

// ---------------------------------------------------------------------------
// Transaktion mit Fortschrittszeilen
// ---------------------------------------------------------------------------

fn einstellungen(o: &Optionen) -> Einstellungen {
    if o.schnell {
        Einstellungen {
            sperre_wiederholung: Duration::from_secs(1),
            sperre_hoechstdauer: Duration::from_secs(10),
            zeit_ohne_fortschritt: Duration::from_secs(5),
            daemon_wartezeit: Duration::from_millis(300),
            fremde_transaktionen_hoechstdauer: Duration::from_secs(10),
            abbruch_frist: Duration::from_secs(3),
            ..Default::default()
        }
    } else {
        Einstellungen::default()
    }
}

async fn fuehre_aus(
    quelle: Rc<dyn Paketquelle>,
    laufzeit: Rc<dyn Laufzeit>,
    o: &Optionen,
    args: &[String],
) -> i32 {
    let vorgang = match vorgang_lesen(args.first()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("qs-cli: {e}");
            return EXIT_AUFRUF;
        }
    };
    if args.len() < 2 && vorgang != Vorgang::QuellenAuffrischen {
        eprintln!("qs-cli: fuehre-aus braucht mindestens ein Paket.");
        return EXIT_AUFRUF;
    }
    let pakete = match pakete_aufloesen(&*quelle, &args[1..]).await {
        Ok(p) => p,
        Err(f) => {
            fehler_zeigen(&f);
            return EXIT_FEHLER;
        }
    };
    let monitor = Monitor::mit(laufzeit, einstellungen(o), Rc::new(Protokoll::standard()));
    if let Some(p) = monitor.protokoll().pfad() {
        eprintln!("qs-cli: Protokoll {}", p.display());
    }
    let (tx, rx) = mpsc::unbounded::<Zustand>();
    let griff = monitor.starten(
        Auftrag { vorgang, pakete, anwendung: None },
        quelle,
        Box::new(move |z| {
            let _ = tx.unbounded_send(z);
        }),
    );
    zustaende_verfolgen(&monitor, griff, rx, o).await
}

async fn laufende(quelle: Rc<dyn Paketquelle>, laufzeit: Rc<dyn Laufzeit>, o: &Optionen) -> i32 {
    match quelle.laufende_transaktionen().await {
        Ok(l) => {
            println!("{} laufende Transaktion(en)", l.len());
            for p in &l {
                println!("  {p}");
            }
            if l.is_empty() {
                return EXIT_OK;
            }
        }
        Err(f) => {
            fehler_zeigen(&f);
            return EXIT_FEHLER;
        }
    }
    let monitor = Monitor::mit(laufzeit, einstellungen(o), Rc::new(Protokoll::standard()));
    let (tx, rx) = mpsc::unbounded::<Zustand>();
    let mut griffe = monitor.wieder_anbinden(
        quelle,
        Box::new(move |z| {
            let _ = tx.unbounded_send(z);
        }),
    );
    let griff = griffe.remove(0);
    zustaende_verfolgen(&monitor, griff, rx, o).await
}

async fn zustaende_verfolgen(
    monitor: &Monitor,
    griff: TransaktionsGriff,
    mut rx: mpsc::UnboundedReceiver<Zustand>,
    o: &Optionen,
) -> i32 {
    let griff = Rc::new(griff);
    let start = Instant::now();
    // Abbruch nach fester Zeit (zum Vorführen und für Integrationstests).
    if let Some(sek) = o.abbrechen_nach {
        let g = griff.clone();
        glib::MainContext::default().spawn_local(async move {
            glib::timeout_future(Duration::from_secs_f64(sek)).await;
            eprintln!("qs-cli: Abbruch nach {sek} s angefordert (--abbrechen-nach)");
            g.abbrechen();
        });
    }
    let letzte = RefCell::new(String::new());
    // Gibt eine Zeile aus, wenn sie sich von der vorigen unterscheidet; `true` = ausgegeben.
    let zeile = |text: &str| -> bool {
        if *letzte.borrow() == text {
            return false;
        }
        println!("[#{} {:>6.1} s] {}", griff.id, start.elapsed().as_secs_f64(), text);
        let _ = std::io::stdout().flush();
        *letzte.borrow_mut() = text.to_string();
        true
    };
    while let Some(z) = rx.next().await {
        match &z {
            Zustand::Simuliert(sim) => {
                let _ = zeile(&format!(
                    "Simuliert: +{} zusätzlich, −{} entfernt, ↑{} aktualisiert, Download {}",
                    sim.installieren.len(),
                    sim.entfernen.len(),
                    sim.aktualisieren.len(),
                    groesse(sim.download_bytes)
                ));
                simulation_zeigen(sim, "             ");
            }
            Zustand::WartetAufBestaetigung => {
                let _ = zeile("Wartet auf Bestätigung");
                let ja = if o.ja {
                    println!("             → bestätigt (--ja)");
                    true
                } else if o.nein {
                    println!("             → verweigert (--nein)");
                    false
                } else {
                    frage("             Fortfahren? [j/N] ")
                };
                if ja {
                    griff.bestaetigen();
                } else {
                    griff.abbrechen();
                }
            }
            Zustand::Laeuft { prozent, status } => {
                let p = prozent.map(|p| format!("{p:>3} %")).unwrap_or_else(|| "  – ".into());
                if zeile(&format!("Läuft {p}  {status}")) && ist_nachfrage(&z) {
                    println!("             (qs-cli wartet weiter; die Oberfläche bietet hier „Abbrechen“)");
                }
            }
            Zustand::WartetAufSperre { seit } => {
                let _ = zeile(&format!(
                    "Wartet auf Paketsperre (seit {:.0} s); Wiederholung alle {} s, höchstens {} s",
                    seit.elapsed().as_secs_f64(),
                    monitor.einstellungen().sperre_wiederholung.as_secs(),
                    monitor.einstellungen().sperre_hoechstdauer.as_secs()
                ));
            }
            Zustand::Fertig => {
                if monitor.braucht_neustart(griff.id) {
                    let _ = zeile("Fertig – Neustart erforderlich");
                } else {
                    let _ = zeile("Fertig");
                }
            }
            Zustand::Fehlgeschlagen(f) => {
                let _ = zeile(&format!("Fehlgeschlagen: {}", fehlertexte::kurz(f)));
                fehler_zeigen(f);
            }
            Zustand::Abgebrochen => {
                let _ = zeile("Abgebrochen");
            }
        }
        if ist_ende(&z) {
            return match z {
                Zustand::Fertig => EXIT_OK,
                Zustand::Abgebrochen => EXIT_ABGEBROCHEN,
                _ => EXIT_FEHLER,
            };
        }
    }
    EXIT_FEHLER
}

fn frage(text: &str) -> bool {
    print!("{text}");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    if std::io::stdin().lock().read_line(&mut s).is_err() {
        return false;
    }
    matches!(s.trim().to_lowercase().as_str(), "j" | "ja" | "y" | "yes")
}

// ---------------------------------------------------------------------------
// Ausgabe
// ---------------------------------------------------------------------------

fn simulation_zeigen(sim: &Simulation, einzug: &str) {
    let leer = sim.installieren.is_empty() && sim.entfernen.is_empty() && sim.aktualisieren.is_empty();
    if leer {
        println!("{einzug}keine zusätzlichen Änderungen");
    }
    for (zeichen, liste) in [("+", &sim.installieren), ("−", &sim.entfernen), ("↑", &sim.aktualisieren)] {
        for p in liste.iter().take(20) {
            println!("{einzug}{zeichen} {} {}", p.name, p.version);
        }
        if liste.len() > 20 {
            println!("{einzug}  … und {} weitere", liste.len() - 20);
        }
    }
    if einzug.is_empty() {
        println!("Download: {}", groesse(sim.download_bytes));
    }
}

fn fehler_zeigen(f: &Fehler) {
    let t = fehlertexte::fehlertext(f);
    eprintln!("── {} ── [{}]", t.ueberschrift, fehlertexte::packagekit_code(f));
    eprintln!("{}", t.erklaerung);
    eprintln!("Empfehlung: {}", t.handlung);
}

fn fehlertexte_zeigen(markdown: bool) {
    if markdown {
        print!("{}", fehlertexte::tabelle_markdown());
        return;
    }
    for (name, f) in fehlertexte::alle_varianten() {
        let t = fehlertexte::fehlertext(&f);
        println!("{name} [{}]", fehlertexte::packagekit_code(&f));
        println!("  Überschrift: {}", t.ueberschrift);
        println!("  Erklärung:   {}", t.erklaerung);
        println!("  Handlung:    {}", t.handlung);
        println!();
    }
}
