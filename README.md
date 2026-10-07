# hassdeck

Demone headless per Raspberry Pi che collega uno stream controller Soomfon XF-CN001
(15 tasti LCD, hardware OEM HOTSPOTEK) a Home Assistant e altre azioni programmabili,
con un'interfaccia web per caricare le icone e configurare i tasti.

Vedi [ANALYSIS.md](./ANALYSIS.md) per l'analisi tecnica, le decisioni di scope e i prossimi passi.

## ⚠️ Sicurezza

L'interfaccia web è protetta da una password. Al primo accesso la pagina `/login` chiede di
crearla: **impostala subito dopo l'installazione**, perché finché manca chiunque raggiunga
l'interfaccia può sceglierla. La password è salvata solo come hash argon2 (`[web] password_hash` in
`config.toml`). Si cambia da `/impostazioni`; per reimpostarla, se la dimentichi, cancella quella riga
e riavvia il servizio. Le sessioni durano 7 giorni e si perdono a ogni riavvio del servizio.

Chi entra può cambiare le azioni dei tasti, eseguirle, sostituire le icone e modificare url e token di
Home Assistant. Il token (che dà pieno controllo della tua istanza Home Assistant) non viene mai
mostrato dall'interfaccia, ma è salvato in chiaro in `config.toml`.

- Di default l'interfaccia è in ascolto solo su `127.0.0.1:8080`, cioè raggiungibile solo dal Raspberry
  stesso (es. via tunnel SSH: `ssh -L 8080:localhost:8080 utente@raspberry`).
- Per raggiungerla dalla rete locale imposta esplicitamente in `config.toml`:
  ```toml
  [web]
  bind = "0.0.0.0:8080"
  ```
  Fallo **solo su una LAN fidata**, e **non esporre mai la porta su internet** (niente port forwarding
  o reverse proxy pubblico): l'interfaccia usa HTTP in chiaro, quindi password e cookie di sessione
  viaggiano non cifrati.
- Il cookie di sessione è `HttpOnly` e `SameSite=Strict`, quindi una pagina web di un altro sito
  aperta nel browser non può usare la tua sessione per inviare richieste all'interfaccia.
- Il servizio systemd gira come utente di sistema non privilegiato `hassdeck`; l'accesso al
  dispositivo USB (`/dev/hidraw*`) è concesso da una regola udev dedicata.
- `config.toml` contiene il token e l'hash della password: deve appartenere all'utente `hassdeck`
  con permessi `600` (vedi installazione). Per leggerlo o modificarlo a mano serve `sudo`.

## Installazione sul Raspberry Pi

```
sudo git clone https://github.com/emyfr/hassdeck.git /opt/hassdeck
sudo chown -R $USER: /opt/hassdeck
cd /opt/hassdeck
cargo build --release
cp config.example.toml config.toml   # e modifica url/token Home Assistant, o usa la pagina /impostazioni

# utente di sistema per il servizio, proprietario dei soli file che scrive
sudo useradd --system --user-group --no-create-home --home-dir /opt/hassdeck \
  --shell /usr/sbin/nologin hassdeck
mkdir -p icons
sudo chown -R hassdeck:hassdeck config.toml icons
sudo chmod 600 config.toml

# accesso al deck (VID:PID 1500:3003) per il gruppo hassdeck
sudo cp systemd/60-hassdeck.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=hidraw

sudo cp systemd/hassdeck.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now hassdeck.service
```

Poi apri subito `http://<raspberry>:8080/login` e crea la password dell'interfaccia (vedi Sicurezza).

Su un Raspberry Pi con alimentazione al limite la compilazione può causare undervoltage e bloccarlo:
in quel caso compila con `cargo build --release -j 1`.

Il file di unit in `systemd/hassdeck.service` assume che il progetto sia in
`/opt/hassdeck` — adatta i percorsi (`WorkingDirectory`, `ExecStart`, `ReadWritePaths`) se diverso.
Il servizio riparte automaticamente in caso di crash (`Restart=on-failure`) e i log sono consultabili con:

```
journalctl -u hassdeck.service -f
```

## Aggiornamento

`config.toml` e `icons/` non sono tracciati da git, quindi un aggiornamento non li tocca:

```
cd /opt/hassdeck
git pull
cargo build --release
sudo systemctl restart hassdeck.service
```

Se cambia il file di unit o la regola udev, ricopiali come nell'installazione (seguiti da
`sudo systemctl daemon-reload` o `sudo udevadm control --reload`).

## Licenza

Distribuito sotto licenza [GNU GPL v3.0 o successiva](./LICENSE).
