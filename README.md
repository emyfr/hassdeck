# streamdeck-ha-bridge

Demone headless per Raspberry Pi che collega uno stream controller Soomfon XF-CN001
(15 tasti LCD, hardware OEM HOTSPOTEK) a Home Assistant e altre azioni programmabili,
con un'interfaccia web per caricare le icone e configurare i tasti.

Vedi [ANALYSIS.md](./ANALYSIS.md) per l'analisi tecnica, le decisioni di scope e i prossimi passi.

## Installazione sul Raspberry Pi

```
cargo build --release
cp config.example.toml config.toml   # e modifica url/token Home Assistant, o usa la pagina /impostazioni

sudo cp systemd/streamdeck-ha-bridge.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now streamdeck-ha-bridge.service
```

Il file di unit in `systemd/streamdeck-ha-bridge.service` assume che il progetto sia clonato in
`/home/<utente>/streamdeck-ha-bridge` — adatta i percorsi (`WorkingDirectory`, `ExecStart`) se diverso.
Il servizio riparte automaticamente in caso di crash (`Restart=on-failure`) e i log sono consultabili con:

```
journalctl -u streamdeck-ha-bridge.service -f
```
