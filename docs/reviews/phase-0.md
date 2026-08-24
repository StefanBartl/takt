# Review Phase 0 — Performance und Sicherheit

*Erstellt 2026-08-23, offen bis zum Abschluss von Phase 0.*

Jede Phase endet mit einem Review, das drei Fragen beantwortet: Was hat die Architektur gelernt? Wo liegen jetzt die Performance-Grenzen? Welche Angriffsfläche ist neu entstanden? Dieses Dokument wird während der Phase fortgeschrieben, nicht erst am Ende geschrieben.

---

## Table of content

  - [Performance](#performance)
  - [Sicherheit und Angriffsflaeche](#sicherheit-und-angriffsflaeche)
  - [Was in dieser Phase bereits entschieden wurde](#was-in-dieser-phase-bereits-entschieden-wurde)
  - [Offene Punkte fuer Phase 1](#offene-punkte-fuer-phase-1)

---

## Performance

### Frame-Eigentümerschaft — entschieden, umgesetzt

Der teuerste vermeidbare Fehler wäre gewesen, Frames als `Vec<u8>` mit Aufrufer-Eigentum zu modellieren und den Pool später nachzurüsten. Ab Phase 1 ist ein Frame ein Platz in einem begrenzten Decoder-Ringpuffer; die Rückgabepflicht durchdringt dann die gesamte Pipeline.

Umgesetzt über `PooledFrame` mit Rückgabe beim `Drop` und `FramePool` mit harter Kapazitätsgrenze. **Ein erschöpfter Pool ist ein Messwert, kein Fehlerfall zum Wegabstrahieren** — er bedeutet, dass der Verbraucher nicht nachkommt. Stilles Nachallozieren würde genau die Information zerstören, die man braucht.

### Shared Memory — noch nicht relevant, aber vorgezeichnet

IPC gibt es in Phase 0 nicht. Ab Phase 2 (Encoder-Hilfsprozess) und Phase 4 (VST3-Bridge) schon, und dann gilt: Audio-Puffer über Shared Memory, Steuerung über einen getrennten Kanal. Der Grund ist Latenz — ein Audio-Puffer durch eine Pipe zu schicken kostet einen Kopiervorgang, den man bei 2,7 ms Budget nicht hat.

Was jetzt bereits stimmen muss: `AudioBuffer` ist **verschachtelt**, nicht planar, weil alle Betriebssystem-Schnittstellen verschachtelt arbeiten und eine Umsortierung an der Prozessgrenze ein vermeidbarer Kopiervorgang wäre.

### Scheduler und Thread-Priorität — Fallstrick identifiziert

Der Audio-Thread braucht erhöhte Priorität. Unter Linux erfordert `SCHED_FIFO` `CAP_SYS_NICE` oder eine Regel in `limits.conf`; auf einer nicht vorbereiteten Installation schlägt die Anforderung fehl.

Das ist der klassische Fall „funktioniert bei mir": auf einem für Audio konfigurierten Arch läuft es, auf Standard-Ubuntu nicht — und man sucht die Ursache von Xruns wochenlang im eigenen Code.

**Festgelegt:** Priorität anfordern, Fehlschlag erkennen, sichtbar melden, mit normaler Priorität weiterlaufen. Niemals stumm scheitern.

### Extrapolationsgrenze — bewusste Entscheidung gegen Bequemlichkeit

Die Audio-Master-Uhr interpoliert zwischen Callbacks mit der Systemuhr. Naheliegend wäre unbegrenzte Extrapolation. Umgesetzt ist eine Grenze bei etwa zwei Callback-Perioden: bleibt der Anker länger aus, hängt der Audio-Thread, und eine weiter extrapolierte Position wäre eine Behauptung ohne Grundlage. Stattdessen bleibt die Uhr stehen und `anchor_age_nanos()` macht das Problem sichtbar.

### Seqlock statt Mutex — gemessen, nicht vermutet

Der Nebenläufigkeitstest `nebenlaeufig_niemals_zerrissen` prüft über 200.000 Schreibvorgänge, dass die Invariante `nanos == samples * 1000` bei keinem Lesevorgang bricht. Ein `Mutex` hätte dieselbe Korrektheit geliefert, aber die Realtime-Invariante verletzt.

### Was noch nicht gemessen ist

* CPU-Last und Speicherbandbreite unter echter Last — es gibt noch keine echte Dekodierung.
* Callback-Laufzeit als Perzentile. **Der Median ist bedeutungslos; das 99,99. Perzentil entscheidet.** Braucht `takt-audio`.
* Startzeit der Anwendung. Der Player-Modus soll schlank starten; das ist derzeit eine Absicht ohne Zahl.

---

## Sicherheit und Angriffsflaeche

### Die Angriffsfläche ist von Anfang an groß

Das muss klar benannt werden: **Mediendateien sind ein klassischer Angriffsvektor.** Ein Demuxer, der auf einer manipulierten Datei läuft, ist exakt das Ziel, für das ein großer Teil aller FFmpeg-CVEs geschrieben wird. Container-Parser sind Zustandsmaschinen über nicht vertrauenswürdige Byteströme, in C geschrieben, mit jahrzehntelang gewachsenen Sonderfällen.

Ein Player ist damit sicherheitstechnisch näher an einem Browser als an einem Textverarbeitungsprogramm. Eine Datei zu öffnen, die man aus dem Internet hat, ist eine Vertrauensentscheidung — und die Software sollte sie nicht schlechter machen, als sie sein muss.

### Maßnahmen, die jetzt greifen

* **`#![forbid(unsafe_code)]` in allen Zone-1-Crates.** `takt-core` und `takt-clock` enthalten kein `unsafe`. Das ist billig, solange es keine FFI gibt — und genau deshalb wird es jetzt festgeschrieben, bevor es unbequem wird.
* **Kein Panic-Pfad in der Datenverarbeitung.** `Rational::new` und `Timestamp::rescale` geben `Option` zurück statt zu panicken. `Rational::lit` panickt und ist ausdrücklich nur für Konstanten im Quelltext dokumentiert, nie für Daten aus Dateien. Ein `panic!` auf eine manipulierte Datei ist eine Denial-of-Service-Lücke.
* **Überlaufsicherheit durchgehend.** Zeitumrechnungen laufen über `i128`, Additionen über `checked_add`, Histogramm-Zähler über `saturating_add`. Ein absichtlich absurder Zeitstempel in einer Datei darf keine arithmetische Panik auslösen.
* **Geometrie wird validiert.** `VideoFrame::alloc` lehnt Nullgrößen ab und rechnet Chroma-Ebenen mit `div_ceil`, statt ungerade Abmessungen falsch zu runden. Falsch berechnete Ebenengrößen sind die Standardquelle für Pufferüberläufe in Videocode.
* **`cargo-deny` in der CI** — auch als Sicherheitsprüfung, nicht nur für Lizenzen: `advisories` fängt bekannte Verwundbarkeiten in Abhängigkeiten.

### Der Prozessschnitt fällt mit dem Lizenzschnitt zusammen

Die wichtigste Erkenntnis dieses Reviews: **Demuxing ist langfristig ein Kandidat für einen eigenen Sandbox-Prozess** — und das ist genau derselbe Schnitt, den die Lizenz-Topologie schon aus anderen Gründen zieht.

```
Hauptanwendung
     |  IPC: dekodierte Frames über Shared Memory
     v
Demux- und Decode-Prozess (reduzierte Rechte, kein Netzwerk, kein Dateisystem
außer der geöffneten Datei)
     |
     v  linkt FFmpeg
nicht vertrauenswürdige Datei
```

Drei Gewinne aus einer Grenze: Lizenz-Isolation, Crash-Isolation, Sandbox. Chrome und Firefox machen es beim Media-Decoding genauso, und dort aus reiner Sicherheitsmotivation.

Für Phase 0 ist das nur eine Notiz. Aber die `VideoBackend`-Schnittstelle ist bereits so geschnitten, dass ein Prozesswechsel dahinter möglich bleibt: sie tauscht Frames und Zeitstempel aus, keine Zeiger auf Interna.

### Was noch nicht adressiert ist

* **Untertitel.** `.ass`-Dateien sind ein eigener Angriffsvektor mit eigener Skriptsprache. Wenn libass eingebunden wird, gehört es in dieselbe Sandbox wie der Demuxer.
* **Netzwerkquellen.** Ab HTTP, HLS oder RTSP kommt TLS-Zertifikatsprüfung dazu, plus die Frage, welche Protokolle überhaupt erlaubt sind. FFmpeg unterstützt mehr Protokolle als man einem Player geben will — eine Allowlist ist Pflicht.
* **Pfadbehandlung.** Playlisten mit relativen Pfaden dürfen nicht aus dem erwarteten Verzeichnis herausführen.
* **Fuzzing.** Sobald ein echter Demuxer existiert, gehört `cargo-fuzz` auf die Parser-Schicht. Vorher gibt es nichts zu fuzzen.

---

## Was in dieser Phase bereits entschieden wurde

| Entscheidung | Grund | Revidierbar? |
|---|---|---|
| Zeit rational, nie `f64` | Drift bei 24000/1001 und 44,1 kHz | nein, durchdringt alles |
| Frame-Pool mit Rückgabepflicht ab Commit eins | Hardware-Decoder ab Phase 1 | nein |
| Genau ein hart-echtzeitfähiger Thread | Audio-Deadline ist hart, Video-Deadline weich | nein |
| Seqlock für den Anker, kein Mutex | Audio-Thread darf nicht blockieren | ja, aber sinnlos |
| Cross-Platform ab Commit eins, CI auf drei Zielen | Linux-Unterstützung als Alleinstellungsmerkmal | nein |
| `NullBackend` als dauerhaftes Werkzeug | Cross-Platform ohne Systemabhängigkeiten, Fehlerzuordnung | – |
| `forbid(unsafe_code)` in Zone 1 | jetzt billig, später unbequem | wäre ein Rückschritt |

---

## Offene Punkte fuer Phase 1

1. **Vulkan Video kann nicht die universelle Antwort sein.** macOS hat kein `VK_KHR_video_decode`; MoltenVK bildet Vulkan auf Metal ab, ohne die Video-Erweiterungen. VideoToolbox ist dort nicht eine Option, sondern der einzige Weg. Die Backend-Abstraktion in `takt-decode` bleibt damit zwingend, mit Vulkan Video als bevorzugtem, nicht als einzigem Pfad.
2. **Der Software-Pfad wird zuerst gebaut.** Er ist der Fallback, er läuft auf dem zehn Jahre alten Testgerät, und er ist die Referenz für die Korrektheit des Hardware-Pfads.
3. **Zero-Copy messen, nicht behaupten.** Zielgröße ist CPU-Last bei 4K60 AV1 gegen `mpv --hwdec=auto`. Ohne diese Zahl ist „Zero-Copy" eine Absichtserklärung.
4. **`assert_no_alloc` in den Audio-Callback**, sobald `takt-audio` existiert. Danach nie wieder entfernen.
5. **Erste `unsafe`-Blöcke werden kommen** — FFI ist ohne sie nicht möglich. Jeder braucht einen `// SAFETY:`-Kommentar, der die Invariante benennt, nicht die Absicht wiederholt.
