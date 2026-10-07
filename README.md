# streamdeck-ha-bridge

Demone headless per Raspberry Pi che collega uno stream controller Soomfon XF-CN001
(15 tasti LCD, hardware OEM HOTSPOTEK) a Home Assistant e altre azioni programmabili,
con un'interfaccia web per caricare le icone e configurare i tasti.

Vedi [ANALYSIS.md](./ANALYSIS.md) per l'analisi tecnica, le decisioni di scope e i prossimi passi.

## ⚠️ Sicurezza

**L'interfaccia web non ha autenticazione.** Chiunque la raggiunga può cambiare le azioni dei tasti,
eseguirle, sostituire le icone e modificare url e token di Home Assistant. Il token (che dà pieno
controllo della tua istanza Home Assistant) non viene mai mostrato dall'interfaccia, ma è salvato in
chiaro in `config.toml`.

- Di default l'interfaccia è in ascolto solo su `127.0.0.1:8080`, cioè raggiungibile solo dal Raspberry
  stesso (es. via tunnel SSH: `ssh -L 8080:localhost:8080 utente@raspberry`).
- Per raggiungerla dalla rete locale imposta esplicitamente in `config.toml`:
  ```toml
  [web]
  bind = "0.0.0.0:8080"
  ```
  Fallo **solo su una LAN fidata**, e **non esporre mai la porta su internet** (niente port forwarding
  o reverse proxy pubblico).
- Anche su una LAN fidata, una pagina web malevola aperta da un browser della stessa rete può inviare
  richieste all'interfaccia (es. "premi tasto"). Evita di associare ai tasti azioni sensibili come
  l'apertura di serrature (`lock.unlock`) finché non sarà disponibile l'autenticazione.
- Il servizio systemd gira come utente di sistema non privilegiato `streamdeck`; l'accesso al
  dispositivo USB (`/dev/hidraw*`) è concesso da una regola udev dedicata.
- Proteggi `config.toml`: `chmod 600 config.toml`.

## Installazione sul Raspberry Pi

```
sudo git clone <url-del-repo> /opt/streamdeck-ha-bridge
sudo chown -R $USER: /opt/streamdeck-ha-bridge
cd /opt/streamdeck-ha-bridge
cargo build --release
cp config.example.toml config.toml   # e modifica url/token Home Assistant, o usa la pagina /impostazioni

# utente di sistema per il servizio, proprietario dei soli file che scrive
sudo useradd --system --user-group --no-create-home --home-dir /opt/streamdeck-ha-bridge \
  --shell /usr/sbin/nologin streamdeck
mkdir -p icons
sudo chown -R streamdeck:streamdeck config.toml icons
sudo chmod 600 config.toml

# accesso al deck (VID:PID 1500:3003) per il gruppo streamdeck
sudo cp systemd/60-streamdeck-ha-bridge.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=hidraw

sudo cp systemd/streamdeck-ha-bridge.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now streamdeck-ha-bridge.service
```

Il file di unit in `systemd/streamdeck-ha-bridge.service` assume che il progetto sia in
`/opt/streamdeck-ha-bridge` — adatta i percorsi (`WorkingDirectory`, `ExecStart`), `ReadWritePaths`) se diverso.
Il servizio riparte automaticamente in caso di crash (`Restart=on-failure`) e i log sono consultabili con:

```
journalctl -u streamdeck-ha-bridge.service -f
```

## Licenza

Distribuito sotto licenza [GNU GPL v3.0 o successiva](./LICENSE).
