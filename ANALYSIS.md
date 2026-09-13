# Analisi: integrazione Soomfon XF-CN001 con Home Assistant

## Obiettivo

Programmare uno stream controller **Soomfon XF-CN001** (15 tasti LCD) sotto Linux in modo da:

1. far eseguire ai tasti azioni verso **Home Assistant** (accendere/spegnere entità, attivare scene, ecc.);
2. mostrare sui tasti (che hanno un mini-schermo LCD ciascuno, non keycap stampati) **icone dinamiche**, idealmente riflettendo lo stato reale delle entità HA (es. luce accesa/spenta).

Il dispositivo è collegato via USB a una macchina Linux (Debian, Raspberry Pi).

## Identificazione del dispositivo

Dal descrittore USB letto sul dispositivo collegato:

```
idVendor  0x1500  (stringa iManufacturer: "HOTSPOTEKUSB")
idProduct 0x3003  (stringa iProduct: "HOTSPOTEKUSB HID DEMO")
bcdDevice 0.02
```

Ricerche su modello e produttore confermano:

- **Brand venduto**: Soomfon XF-CN001, "15 LCD Macro Keys Stream Controller" (Amazon, eBay).
- **OEM reale**: HOTSPOTEK — un produttore cinese che fabbrica lo stesso hardware rivenduto sotto più brand (Mirabox, Ajazz, Soomfon). Il progetto open source `mirajazz` cita esplicitamente Soomfon come rebrand della serie Mirabox 293S.
- Il VID:PID `1500:3003` **è in realtà già catalogato**: il plugin [opendeck-akp153](https://github.com/4ndv/opendeck-akp153) (v0.16.2 di `mirajazz`) lo mappa esplicitamente come `Kind::SFSTC` ("Soomfon Stream Controller", costanti `SF_STC_VID = 0x1500` / `SF_STC_PID = 0x3003`), con `protocol_version = 3`. La prima analisi (basata su ricerca web) non aveva trovato questo riscontro — verificato successivamente leggendo il codice sorgente del plugin. Vedi anche "Test empirici svolti" più sotto: il codice esiste ma non basta da solo a far funzionare il dispositivo così com'è.

## Struttura USB del dispositivo

Due interfacce HID:

| Interfaccia | Classe/Sottoclasse | Endpoint | Funzione presunta |
|---|---|---|---|
| 0 | HID vendor-specific | EP2 IN (interrupt, 512B) / EP3 OUT (interrupt, 1024B) | canale dati proprietario, pacchetti grandi → verosimilmente upload immagini/icone sui tasti + eventuale configurazione |
| 1 | HID Boot Keyboard | EP1 IN (interrupt, 8B) | tastiera HID standard: i tasti, se già configurati, inviano keystroke reali al sistema operativo |

Punto chiave (assunzione iniziale, **non confermata empiricamente** — vedi "Test empirici svolti"): l'interfaccia 1 dovrebbe far sì che la pressione dei tasti funzioni "out of the box" su qualunque Linux, senza driver custom, perché il dispositivo si comporta come una tastiera USB qualunque. Sul dispositivo reale testato, però, la pressione fisica dei tasti non produce alcun evento né sul canale vendor né sull'interfaccia tastiera (kernel `evdev`) — da investigare ulteriormente.

## Ecosistema open source rilevante

- **[mirajazz](https://github.com/4ndv/mirajazz)** (Rust) — libreria che implementa il protocollo HOTSPOTEK per upload immagini e lettura tasti (incluso long-press) su dispositivi Mirabox/Ajazz/rebrand. Richiede di registrare manualmente VID/PID e regole udev per ogni dispositivo non ancora in elenco.
- **[OpenDeck](https://github.com/nekename/OpenDeck)** — applicazione Linux/Windows/macOS con sistema a plugin (compatibile con plugin scritti per l'SDK Elgato + API OpenAction proprie), usa backend come mirajazz per i dispositivi non-Elgato.
- **StreamController** — alternativa con plugin Home Assistant nativo già esistente (controllo/lettura stato entità configurabile in YAML).

Nessuno dei tre risulta avere, ad oggi, un profilo dispositivo pronto per VID `0x1500`/PID `0x3003`.

## Ambiente di destinazione

Il dispositivo è collegato a un Raspberry Pi headless (architettura `aarch64`, Debian 13 "trixie", kernel `rpt-rpi-2712` → SoC Pi 5), senza ambiente grafico attivo (`graphical.target` inactive, sessione solo `tty`) e senza toolchain Rust installata al momento.

Questo esclude di fatto **OpenDeck** come base per l'implementazione: è un'applicazione desktop (Tauri + webview), pensata per girare con un'interfaccia grafica, e non ha supporto ARM/Raspberry documentato. Non ha senso installare un ambiente grafico su questo Pi solo per farlo girare.

**mirajazz**, essendo una semplice libreria Rust senza dipendenze grafiche, è invece compatibile con un demone headless — richiede solo l'installazione del toolchain Rust (`cargo`) sul Pi.

Conclusione: la base realistica per l'implementazione è un **demone custom headless basato su (o ispirato a) mirajazz**, non un'app come OpenDeck.

## Valutazione di fattibilità

| Requisito | Difficoltà | Note |
|---|---|---|
| Tasto → azione su Home Assistant | **Bassa** | Il tasto emette già un keystroke reale (interfaccia 1). Basta un demone che ascolta l'input (es. via `evdev`/`hidraw`) e traduce le combinazioni di tasti in chiamate REST/WebSocket verso l'API di Home Assistant. Non serve reverse engineering del protocollo vendor. |
| Icone dinamiche sui tasti | **Medio-alta** | Richiede parlare con l'interfaccia 0 (canale vendor). Il pattern di endpoint (IN 512B / OUT 1024B) è coerente con quello già documentato per la famiglia HOTSPOTEK/Mirabox, quindi c'è una probabilità concreta che il formato immagine sia identico o quasi a quello già implementato in `mirajazz` per la serie 293S — ma va verificato empiricamente, non assunto. |

### Percorsi possibili per le icone

1. **Tentativo diretto**: provare il protocollo 293S di `mirajazz` così com'è contro questo dispositivo (stesso formato pacchetto immagine, stesso VID range OEM), verificando se risponde correttamente. Costo basso, esito incerto.
2. **Cattura del traffico USB ufficiale**: se (1) non funziona, catturare con Wireshark + USBPcap il traffico generato dall'app Windows/macOS Soomfon mentre carica un'icona su un tasto, per ricavare il formato esatto dei pacchetti (intestazioni, formato immagine — RGB565/JPEG/altro, dimensioni, sequenza di comandi). Costo più alto (serve una macchina Windows/macOS con l'app ufficiale e il dispositivo), ma affidabile.

## Ipotesi: prodotto installabile (Raspberry + deck + interfaccia web)

Scenario alternativo/successivo a quello del semplice demone personale: confezionare **Raspberry Pi + deck** come un'appliance pronta all'uso, con un'**interfaccia web** per caricare le icone sui tasti e programmare le azioni (invece di editare file di configurazione a mano). Cambia sensibilmente l'architettura rispetto al demone minimale descritto sopra.

### Componenti aggiuntivi necessari

| Componente | Ruolo |
|---|---|
| Web server locale (backend) | Espone API per: elenco tasti, upload icona per tasto, definizione azione per tasto, lettura stato corrente |
| Frontend web | UI per caricare/anteprima icone (con eventuale resize/conversione automatica nel formato del deck) e per configurare l'azione associata a ciascun tasto |
| Storage di configurazione | Persistenza locale (file JSON o piccolo DB) di: mappatura tasto→icona, tasto→azione, credenziali/URL di Home Assistant |
| Gestore USB hotplug | Il deck deve essere riconosciuto automaticamente alla connessione (regole udev + servizio che si accorge del plug/unplug), senza intervento manuale, trattandosi di un'appliance |
| Servizio di avvio | Backend + comunicazione col deck devono partire da soli al boot del Raspberry (systemd), senza bisogno di login o azioni manuali — coerente con l'idea di "prodotto" |

### Implicazioni sul modello dati

Serve un concetto esplicito di **profilo tasti**: per ciascun tasto (1-15) → un'icona (immagine caricata dall'utente, convertita nel formato/risoluzione richiesti dal canale vendor) + un'azione. Se in futuro si vogliono più "pagine"/profili richiamabili (come fanno gli Stream Deck originali), va previsto fin da ora anche un concetto di profilo multiplo, non solo una mappatura fissa a 15 slot.

### Decisioni prese su questo scenario

- **Tipo di azioni**: ~~solo Home Assistant~~ → **generico**. Il tasto è associato a un'azione di un tipo tra più possibili (astrazione "action type"); Home Assistant è il primo tipo implementato, ma il modello dati e l'architettura vanno pensati fin da subito per aggiungere altri tipi (es. webhook generico, comando shell locale).
- **Autenticazione web UI**: **minima** — login semplice sull'interfaccia di configurazione (a differenza di altri servizi homelab dell'utente su LAN fidata, qui si prevede comunque un login basilare, pensando anche a una futura distribuzione ad altri).
- **Singolo deck vs fleet**: **singolo deck** per istanza — il modello dati non deve predisporsi a gestire più deck collegati allo stesso Raspberry; scope volutamente ridotto al caso d'uso reale.
- **Distribuzione**: **solo software da installare** — un pacchetto/servizio da mettere su un Raspberry già pronto e già configurato dall'utente, non un'immagine SD preconfezionata.
- **Provisioning iniziale**: ~~wizard web UI vs file precompilato~~ → **entrambi**. Tutta la configurazione (URL/token HA, action type, mappatura tasti) si imposta tramite la web UI e viene salvata in un file di configurazione; il file resta comunque modificabile a mano (es. per backup, versionamento, o modifiche rapide senza passare dalla UI).

## Decisioni da prendere prima di implementare

1. ~~**Ambito del bridge**~~ → **risolto**: demone minimale headless basato su/ispirato a `mirajazz` (nessuna UI). OpenDeck escluso per incompatibilità con un Pi senza ambiente grafico (vedi "Ambiente di destinazione").
2. ~~**Sincronizzazione stato HA → icone**~~ → **risolto per l'MVP**: si parte con icone statiche (una sola icona per azione, nessun riflesso dello stato reale dell'entità). La sincronizzazione in tempo reale (icona diversa se la luce è accesa/spenta, che richiederebbe un canale WebSocket persistente verso HA oltre alla REST call in uscita) resta un'iterazione futura, da affrontare solo dopo aver validato il canale immagini via USB.
3. **Dove gira il demone**: confermato — sul Raspberry Pi a cui il dispositivo è fisicamente collegato, presumibilmente come servizio systemd dedicato (nessun container Docker già presente su questa macchina, da verificare se preferito).
4. **Necessità di cattura USB**: se si decide di procedere subito col tentativo (1) della sezione precedente, si può partire senza altre risorse (serve però installare `cargo`/Rust sul Pi); se serve la cattura del traffico (2), va programmata una sessione con macchina Windows/macOS + app ufficiale Soomfon disponibili.

## Test empirici svolti

Verifica pratica sul dispositivo reale (Raspberry Pi di test), con Rust/`cargo` installato via `rustup` e un piccolo binario di test basato sulla libreria `mirajazz` (path locale `~/mirajazz` sul Pi, clonata da GitHub) collegato al deck reale.

**Setup confermato**: `DeviceQuery::new(65440, 1, 0x1500, 0x3003)` (usage_page/usage_id del canale vendor), `protocol_version = 3`, `KEY_COUNT = 18` (modello a griglia 3×6 mutuato da `opendeck-akp153`, pensato per la famiglia AKP153 con encoder). Il dispositivo risponde con un serial number reale e univoco (`<serial>`), coerente con `protocol_version >= 2` secondo la documentazione di `mirajazz`.

**Scrittura immagini (canale vendor, interfaccia 0)**: `set_brightness`, `clear_all_button_images`, `set_button_image`, `flush` vengono tutti accettati dal dispositivo senza errori (ACK ricevuto). Le immagini **arrivano visivamente su alcuni tasti fisici**, ma:

- Il layout fisico reale è **3 righe × 5 tasti (15 totali)**, non 3×6 come assunto dal modello `KEY_COUNT=18`/`ROW_COUNT×COL_COUNT` di `opendeck-akp153` — più una **barra verticale separata** (a lato, che occupa lo spazio di 3 tasti), che **si accende correttamente** una volta usati gli indici logici giusti (vedi tabella sotto).
- `device.set_mode(0..3)` (funzione generica della libreria, usata da altri dispositivi Mirabox per passare da modalità tastiera a modalità software) viene accettata senza errori su tutti i valori 0-3, ma non è stato isolato se sia necessaria o ininfluente per questo dispositivo — la scrittura immagini funziona comunque senza chiamarla.

**Mappatura indice logico ↔ tasto fisico (confermata empiricamente, tasti numerati 1-15 da sinistra a destra, dall'alto in basso)**:

La libreria usa un modello a 18 slot (`KEY_COUNT=18`, ereditato dalla famiglia AKP153), ma il firmware di questo Soomfon XF-CN001 li assegna **per colonna, partendo dalla colonna più a destra, dall'alto in basso** — non riga per riga come nel modello AKP153 originale. Formula: con tasto fisico `n` (1-15), `row = (n-1) / 5`, `col_in_row = (n-1) % 5`, `group = 4 - col_in_row`, **`indice_logico = group * 3 + row`**.

| Tasto fisico | Indice logico | | Tasto fisico | Indice logico | | Tasto fisico | Indice logico |
|---|---|---|---|---|---|---|---|
| 1 | 12 | | 6 | 13 | | 11 | 14 |
| 2 | 9 | | 7 | 10 | | 12 | 11 |
| 3 | 6 | | 8 | 7 | | 13 | 8 |
| 4 | 3 | | 9 | 4 | | 14 | 5 |
| 5 | 0 | | 10 | 1 | | 15 | 2 |

Gli indici logici **15, 16, 17** (i tre "extra" del modello a 18 slot) corrispondono ai **3 elementi della barra verticale** — confermato acceso correttamente.

Nota aperta: la dimensione immagine per la barra verticale è stata assunta dal codice ereditato da AKP153 (82×82 per l'indice 17, 95×95 per gli indici 15-16, stessa logica delle posizioni "encoder" nel modello originale) — funziona per un riempimento a tinta unita, ma la **risoluzione/aspect ratio reale della barra non è ancora confermata** con un'icona vera (potrebbe apparire distorta/tagliata). Da verificare quando si testeranno icone reali anziché colori pieni.

**Lettura pressione tasti — risolto**: inizialmente, testando sia il canale vendor (`Device::get_reader`) sia l'interfaccia kernel HID-tastiera (`/dev/input/event0`), non arrivava alcun evento nonostante pressioni fisiche confermate. Causa trovata: la libreria `mirajazz` invia un handshake di attivazione (`initialize()`, comandi `DIS`+`LIG`) solo pigramente, la prima volta che si chiama una funzione come `set_brightness()` o `clear_all_button_images()` — se ci si limita a `Device::connect()` seguito subito da `get_reader()`/lettura, il dispositivo non è mai stato "svegliato" e non riporta nulla. Basta chiamare `device.set_brightness(...)` una volta prima di iniziare a leggere.

Con l'handshake, il canale vendor riporta correttamente le pressioni (`data[9]` = indice tasto, `data[10]` = stato 1/0 premuto/rilasciato). **L'indice riportato in lettura non è lo stesso indice logico usato in scrittura**: è sempre `indice_scrittura + 1` (verificato su più tasti: tasto1 scrittura=12→lettura=13, tasto2 scrittura=9→lettura=10, tasto5 scrittura=0→lettura=1). Non ancora testato se la tastiera HID (`/dev/input/event0`) inizi a funzionare anch'essa dopo lo stesso handshake, o se richieda un meccanismo separato.

**Barra verticale — probabilmente solo display, non interattiva**: toccandola durante i test di lettura sono arrivati eventi (indici 13, 14 in lettura), ma corrispondenti a tasti fisici reali distanti tra loro, non coerenti con una posizione sulla barra — probabile che il tocco generi un tipo di report diverso (es. "encoder", non "bottone semplice") che la lettura semplificata usata nei test (solo `data[9]`/`data[10]`) non interpreta correttamente. Non approfondito ulteriormente per ora (bassa priorità): si assume la barra sia **solo un display**, non un input, salvo necessità future — in tal caso servirebbe ispezionare i byte grezzi del pacchetto HID per capire il formato reale del report.

**Problema di alimentazione — risolto**: durante i primi test di scrittura immagini, il Raspberry Pi si è spento/bloccato completamente due volte (richiesto riavvio fisico), con `dmesg` che riportava `hwmon: Undervoltage detected!`. Causa: **alimentatore generico/non originale** insufficiente a reggere il picco di corrente del deck USB durante il rendering. Dopo sostituzione dell'alimentatore, nessun ulteriore warning di undervoltage e test di scrittura ripetuti senza problemi.

## Prossimi passi proposti

1. ~~Creare repo dedicato per il bridge~~ → fatto.
2. ~~Verificare in pratica se `mirajazz` riesce a scrivere un'immagine di test~~ → fatto, con esito positivo.
3. ~~Risolvere il problema di alimentazione~~ → fatto (alimentatore sostituito).
4. ~~Completare la tabella di permutazione indice-logico → tasto-fisico~~ → fatto, vedi tabella sopra (15 tasti + barra verticale, tutti confermati).
5. ~~Investigare perché la pressione dei tasti non produce eventi~~ → fatto: serve l'handshake `set_brightness`/`initialize()` prima di leggere; indice di lettura = indice di scrittura + 1 (vedi sopra).
6. Confermare la risoluzione/aspect ratio reale della barra verticale con un'icona vera (non tinta unita).
7. ~~Verificare se la barra verticale è anche premibile/touch~~ → non approfondito, trattata come solo display per ora (vedi sopra).
8. ~~Progettare il bridge verso Home Assistant~~ → fatto: demone Rust (`src/main.rs`, `src/config.rs`, `src/keymap.rs`, `src/actions.rs`) che carica una config TOML (`config.toml`, esempio in `config.example.toml`), scrive le icone statiche configurate all'avvio, ascolta le pressioni e chiama i servizi Home Assistant via REST (`POST /api/services/<dominio>/<servizio>` con bearer token). Testato end-to-end sul Pi (tasto→azione→chiamata HTTP, incluso il caso "tasto senza azione configurata").
9. ~~Interfaccia web di monitoraggio~~ → fatto: server HTTP integrato nello stesso processo (`src/web.rs`, crate `axum`), sezione `[web]` in config (default bind `0.0.0.0:8080`, nessuna autenticazione per ora — scope volutamente limitato a sola lettura). Pagina singola (`web/index.html`) con griglia che rispecchia il layout fisico (15 tasti 3×5 + barra verticale statica), aggiornata in tempo reale via WebSocket (`/ws`): ogni tasto si illumina alla pressione e lampeggia verde/rosso in base all'esito dell'azione eseguita; stato di base (connessione, serial, ultima azione) via polling su `/api/status` ogni 3s. Testato end-to-end sul deck reale.
10. ~~Editing configurazione da web UI~~ → fatto: nuova pagina `/configura` (`web/configure.html`) con una card per tasto (1-15). Upload icona (`POST /api/keys/{key}/icon`, multipart): salva il file in `icons/`, lo scrive subito sul deck reale e aggiorna+persiste `config.toml`. Assegnazione azione (`PUT /api/keys/{key}/action`, JSON) e rimozione (`DELETE`), con validazione lato server per tipo di azione; le icone caricate sono servite staticamente da `/icons/*` (crate `tower-http`, feature `fs`). La config è ora condivisa in `Arc<RwLock<Config>>` tra il loop principale e i handler web, non più caricata una sola volta all'avvio. Testato end-to-end (upload icona → comparsa reale sul deck, azione salvata → eseguita alla pressione).
11. ~~Icone visibili anche nel monitor~~ → fatto: la griglia di `/index.html` ora recupera `/api/keys` e mostra l'icona reale (se presente) come sfondo del tasto, con un piccolo badge numerico nell'angolo; polling separato ogni 5s.
12. ~~Secondo tipo di azione: chiamata URL generica~~ → fatto: `Action::Url { url, method }` (metodo di default POST), eseguita come richiesta HTTP diretta senza autenticazione Home Assistant. L'editor web permette di scegliere il tipo di azione da un menu a tendina (Home Assistant / Chiamata URL) con i campi corrispondenti. Testato end-to-end con una chiamata GET reale verso un endpoint pubblico.
13. ~~Feedback visivo alla pressione~~ → fatto: le icone caricate vengono decodificate e ridimensionate una sola volta (al caricamento o all'upload), tenendo in cache in memoria (`icons::IconCache`, `Arc<RwLock<HashMap<u8, KeyIcons>>>`) sia la variante a riposo sia una variante "premuta" pre-calcolata. Alla pressione fisica del tasto viene scritta subito la variante premuta, al rilascio quella normale — nessuna rilettura da disco o ricalcolo ad ogni pressione. Primo tentativo con scurimento (poco visibile su schermo LCD piccolo/basso contrasto) sostituito con un **blend di tinta arancione** (65% di intensità), molto più evidente indipendentemente dai colori originali dell'icona. Confermato dall'utente: nessun ritardo percepibile, effetto ben visibile.
14. ~~Sezione impostazioni Home Assistant~~ → fatto: nuova pagina `/impostazioni` (`web/settings.html`) per url/token/verifica-TLS, con salvataggio (`PUT /api/settings/home_assistant`, validazione url) e un pulsante "Verifica connessione" (`POST /api/settings/home_assistant/test`, chiama `GET /api/` su Home Assistant con il token fornito e riporta l'esito). Aggiunto anche `verify_tls` (default `true`) per poter disattivare la verifica del certificato su istanze locali self-signed — le chiamate verso Home Assistant usano un client HTTP dedicato che rispetta questa opzione, distinto dal client generico usato per l'azione "Chiamata URL".
15. ~~Testato con un'istanza Home Assistant reale~~ → fatto: connessione verificata con successo (`"message":"API running."}`) verso un'istanza reale dell'utente. Riscontrato e risolto un problema reale durante il test: un long-lived access token non più valido (probabilmente rigenerato/eliminato dopo la copia) causava 401 Unauthorized — confermato bypassando completamente il nostro codice con una chiamata `curl` diretta dal Pi, per escludere un bug applicativo prima di indagare sul token. Risolto rigenerando il token in Home Assistant.
16. ~~Servizio systemd per avvio automatico~~ → fatto: `systemd/streamdeck-ha-bridge.service` (istruzioni in README.md), `Restart=on-failure` + `RestartSec=3`. Attualmente gira come `User=root` (necessario per l'accesso a `/dev/hidraw*` senza regole udev dedicate — nota di hardening futuro, vedi sotto). Log persistenti via `journalctl -u streamdeck-ha-bridge.service`. Testato: riavvio automatico confermato dopo un `SIGKILL` forzato.
17. **Incidente**: il Raspberry Pi è diventato irraggiungibile (rete e SSH entrambi giù) e ha richiesto un riavvio fisico manuale da parte dell'utente. Causa non determinata: `dmesg` non riportava warning di undervoltage questa volta (a differenza dell'incidente coi alimentatore descritto sopra), memoria/disco risultavano sani al riavvio, ma **il journal di sistema non era persistente** (`Storage=auto` di default, nessun log sopravvissuto al riavvio) — impossibile ricostruire la causa. Corretto impostando `Storage=persistent` in `/etc/systemd/journald.conf` (nota: il flush non ha funzionato immediatamente dopo il solo restart di `systemd-journald`; potrebbe richiedere un riavvio completo per essere pienamente efficace — da verificare al prossimo incidente). Gestione hotplug USB del deck non ancora implementata (se il device si disconnette a runtime, il comportamento non è stato testato).
18. Nota aperta: l'editor web (`/configura`, `/impostazioni`) non ha ancora autenticazione, nonostante la decisione iniziale in questo documento la prevedesse come "minima" — scope rimandato deliberatamente, da rivalutare prima di un'eventuale distribuzione più ampia o esposizione oltre la LAN fidata. Da notare: il token Home Assistant viene attualmente restituito in chiaro da `GET /api/settings/home_assistant` per precompilare il form — accettabile solo finché l'accesso all'editor resta privo di autenticazione e limitato alla LAN fidata.
19. Nota di hardening futura: il servizio systemd gira come `root` per accedere a `/dev/hidraw*`; si potrebbe creare una regola udev dedicata (per VID:PID `1500:3003`) e un utente/gruppo non privilegiato per ridurre la superficie d'attacco.
20. ~~Tasti cliccabili anche da web~~ → fatto: la griglia di `/index.html` è ora cliccabile (`POST /api/keys/{key}/press`) e produce lo stesso comportamento della pressione fisica — stesso lampeggio icona (variante "premuta" mostrata per ~180ms, dato che da web non esiste una vera durata di pressione da tenere) ed esecuzione della stessa azione configurata. Ottenuto estraendo la logica di esecuzione azione in `web::run_key_action`, condivisa tra il loop di lettura tasti fisico e l'handler web — nessuna duplicazione di codice, comportamento garantito identico. Testato end-to-end, incluso con una chiamata reale riuscita verso Home Assistant.
21. ~~Elenco entità Home Assistant per popolare l'editor~~ → fatto: nuovo endpoint `GET /api/home_assistant/entities` (`actions::list_entities`, chiama `GET /api/states` su Home Assistant, estrae `entity_id`+`friendly_name`) — testato con l'istanza reale dell'utente, **815 entità** recuperate correttamente. Nell'editor (`/configura`) il campo `entity_id` è ora un combobox custom con ricerca integrata (non un `<select>` semplice, troppo lungo da scorrere con centinaia di voci; non un `<datalist>` nativo, funzionante ma esperienza giudicata insoddisfacente dall'utente): un input di testo apre una tendina filtrabile in tempo reale (fino a 100 risultati), con fallback a un campo di testo libero se le entità non sono recuperabili (es. Home Assistant non ancora configurato).
