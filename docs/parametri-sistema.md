# Parametri di sistema disponibili

Elenco dei dati del Raspberry Pi che un tasto può mostrare con l'azione **Dato di sistema**
(vedi `src/system.rs`). I valori di esempio sono stati letti su un Raspberry Pi 5 (8 GB, Raspberry Pi
OS basato su Debian 13, kernel 6.18) con ventola attiva e un acceleratore Hailo sul PCIe.

Legenda dello stato:

- ✅ **disponibile**: già selezionabile nell'editor (`/configura`);
- ➕ **da aggiungere**: il dato è leggibile, basta implementarlo in `system.rs`;
- ⚠️ **con riserva**: leggibile solo con permessi aggiuntivi o non sempre presente.

Le sorgenti in `/proc` e `/sys` sono leggibili dall'utente del servizio (`hassdeck`) anche con le
restrizioni della unit systemd. `vcgencmd` richiede il gruppo `video`, già assegnato al servizio con
`SupplementaryGroups=video`. I valori "per secondo" (CPU, rete, disco) vanno calcolati come
differenza tra due letture: il primo campione dopo l'avvio mostra "—".

## Processore

| Parametro | id | Sorgente | Esempio | Stato |
|---|---|---|---|---|
| Utilizzo CPU totale | `cpu_percent` | `/proc/stat` (differenza tra due letture) | 0 % (a riposo) | ✅ |
| Utilizzo per core (0-3) | `cpu<N>_percent` | `/proc/stat`, righe `cpu0`…`cpu3` | 0 % (a riposo) | ➕ |
| Frequenza CPU attuale | `cpu_freq` | `/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq` (kHz) | 2400 MHz | ➕ |
| Frequenza CPU massima | `cpu_freq_max` | `.../cpufreq/scaling_max_freq` | 2400 MHz | ➕ |
| Carico medio 1 / 5 / 15 min | `load_1`, `load_5`, `load_15` | `/proc/loadavg` | 0,78 | ➕ |
| Numero di processi | `processes` | conteggio di `/proc/[0-9]*` | 166 | ➕ |
| Governor della frequenza | `cpu_governor` | `.../cpufreq/scaling_governor` (testo) | ondemand | ➕ |

## Temperature e raffreddamento

| Parametro | id | Sorgente | Esempio | Stato |
|---|---|---|---|---|
| Temperatura CPU (SoC) | `cpu_temp` | `/sys/class/thermal/thermal_zone0/temp` (m°C) | 53,5 °C | ✅ |
| Temperatura chip RP1 (USB, Ethernet) | `rp1_temp` | hwmon `rp1_adc`, `temp1_input` (m°C) | 56,1 °C | ➕ |
| Velocità ventola | `fan_rpm` | hwmon `pwmfan`, `fan1_input` | 3849 rpm | ➕ |
| Livello ventola (0-4) | `fan_level` | `/sys/class/thermal/cooling_device0/cur_state` / `max_state` | 1 / 4 | ➕ |
| Temperatura acceleratore Hailo | `hailo_temp` | `hailortcli` | — | ⚠️ |

Il dispositivo Hailo al momento della lettura non era visibile (`/dev/hailo*` assente, driver non
caricato): la temperatura si potrebbe leggere solo con il driver attivo e passando da `hailortcli`.

## Alimentazione (PMIC del Raspberry Pi 5)

Tutti letti con `vcgencmd pmic_read_adc` (gruppo `video`). Il PMIC espone una tensione e una corrente
per ogni linea di alimentazione interna.

| Parametro | id | Canale PMIC | Esempio | Stato |
|---|---|---|---|---|
| Tensione di ingresso (USB-C) | `supply_voltage` | `EXT5V_V` | 5,13 V | ✅ |
| Corrente del core CPU | `core_current` | `VDD_CORE_A` | 1,07 A | ➕ |
| Tensione del core CPU | `core_voltage` | `VDD_CORE_V` (anche `vcgencmd measure_volts core`) | 0,91 V | ➕ |
| Potenza stimata del core | `core_power` | `VDD_CORE_V` × `VDD_CORE_A` | 0,98 W | ➕ |
| Linea 3,3 V di sistema | `rail_3v3_v`, `rail_3v3_a` | `3V3_SYS_V`, `3V3_SYS_A` | 3,30 V / 0,13 A | ➕ |
| Linea 1,8 V di sistema | `rail_1v8_v`, `rail_1v8_a` | `1V8_SYS_V`, `1V8_SYS_A` | 1,80 V / 0,19 A | ➕ |
| Linea 1,1 V di sistema | `rail_1v1_v`, `rail_1v1_a` | `1V1_SYS_V`, `1V1_SYS_A` | 1,10 V / 0,23 A | ➕ |
| Linea 0,8 V | `rail_0v8_v`, `rail_0v8_a` | `0V8_SW_V`, `0V8_SW_A` | 0,80 V / 0,26 A | ➕ |
| Linea Wi-Fi (3,7 V) | `rail_wifi_v`, `rail_wifi_a` | `3V7_WL_SW_V`, `3V7_WL_SW_A` | 3,66 V / 0,06 A | ➕ |
| Memoria DDR | `ddr_vdd2_v`, `ddr_vddq_v` | `DDR_VDD2_V`, `DDR_VDDQ_V` | 1,11 V / 0,60 V | ➕ |
| Tensione HDMI | `hdmi_voltage` | `HDMI_V` | 5,13 V | ➕ |
| Batteria RTC | `rtc_battery` | `BATT_V` | 0,01 V (assente) | ➕ |
| Stato throttling | `throttled` | `vcgencmd get_throttled` (bit di undervoltage, frequenza limitata, throttling, temperatura) | 0x0 | ➕ |
| Allarme undervoltage | `undervoltage` | hwmon `rpi_volt`, `in0_lcrit_alarm` (0/1) | 0 | ➕ |
| Corrente massima dell'alimentatore | `psu_max_current` | `/proc/device-tree/chosen/power/max_current` (mA) | 3000 mA | ➕ |

La somma delle correnti del PMIC **non** è il consumo totale della scheda: mancano le porte USB
(quindi il deck), la ventola e le periferiche PCIe.

## Frequenze dei clock (VideoCore)

Letti con `vcgencmd measure_clock <nome>`.

| Parametro | id | Clock | Esempio | Stato |
|---|---|---|---|---|
| Clock ARM (CPU) | `clock_arm` | `arm` | 2400 MHz | ➕ |
| Clock core GPU | `clock_core` | `core` | 910 MHz | ➕ |
| Clock 3D (V3D) | `clock_v3d` | `v3d` | 1150 MHz | ➕ |
| Clock ISP / HEVC | `clock_isp`, `clock_hevc` | `isp`, `hevc` | 910 MHz | ➕ |
| Clock HDMI | `clock_hdmi` | `hdmi` | 648 MHz | ➕ |
| Clock scheda SD | `clock_emmc` | `emmc` | 200 MHz | ➕ |

## Memoria

| Parametro | id | Sorgente | Esempio | Stato |
|---|---|---|---|---|
| RAM utilizzata | `memory_percent` | `/proc/meminfo` (`MemTotal` − `MemAvailable`) | 5 % | ✅ |
| RAM disponibile | `memory_available` | `/proc/meminfo`, `MemAvailable` | 7,5 GB | ➕ |
| Swap utilizzata (zram) | `swap_percent` | `/proc/meminfo`, `SwapTotal` − `SwapFree` | 0 % | ➕ |
| Memoria GPU | `gpu_memory` | `vcgencmd get_mem gpu` | 4 MB | ➕ |

## Disco

| Parametro | id | Sorgente | Esempio | Stato |
|---|---|---|---|---|
| Spazio usato sulla SD | `disk_percent` | `statvfs("/")` | 29 % | ➕ |
| Spazio libero sulla SD | `disk_free` | `statvfs("/")` | 38,7 GB | ➕ |
| Letture / scritture al secondo | `disk_read`, `disk_write` | `/proc/diskstats`, riga `mmcblk0` (differenza tra due letture) | 0 kB/s | ➕ |

## Rete

| Parametro | id | Sorgente | Esempio | Stato |
|---|---|---|---|---|
| Traffico Ethernet in ingresso / uscita | `net_rx`, `net_tx` | `/sys/class/net/end0/statistics/rx_bytes`, `tx_bytes` (differenza tra due letture) | 0,6 / 0,5 kB/s | ➕ |
| Traffico Wi-Fi | `wifi_rx`, `wifi_tx` | stessi file per `wlan0` | — | ➕ |
| Stato del link Ethernet | `eth_link` | `/sys/class/net/end0/operstate` (`up`/`down`) | up | ➕ |
| Segnale Wi-Fi | `wifi_signal` | `/proc/net/wireless` | — | ⚠️ |

Il segnale Wi-Fi è disponibile solo se il Raspberry è collegato a una rete Wi-Fi: al momento della
lettura la rete passava dall'Ethernet e `/proc/net/wireless` era vuoto. Il nome dell'interfaccia
cablata è `end0` sul Raspberry Pi 5 (può essere `eth0` su altri modelli).

## Sistema

| Parametro | id | Sorgente | Esempio | Stato |
|---|---|---|---|---|
| Tempo di attività | `uptime` | `/proc/uptime` | 47 min | ➕ |
| Ora corrente | `clock` | orologio di sistema | 23:45 | ➕ |
| Data corrente | `date` | orologio di sistema | 07/10 | ➕ |
| Container Docker in esecuzione | `docker_containers` | `docker ps` | 1 | ⚠️ |

Contare i container richiede l'accesso al socket di Docker, cioè il gruppo `docker`, che equivale a
dare i permessi di root all'utente del servizio: sconsigliato.

## Note per l'implementazione

- Ogni nuovo parametro si aggiunge in `src/system.rs`: una voce in `METRICS` (id, nome, etichetta
  breve, unità, decimali) e la lettura in `read_metric`. L'editor lo propone automaticamente.
- I parametri letti da `vcgencmd` lanciano un processo a ogni lettura (ogni 2 secondi per tasto).
  Se ne servono molti conviene una sola chiamata `vcgencmd pmic_read_adc` per giro, condivisa tra i
  tasti.
- Sul tasto c'è spazio per un'etichetta di 4-5 caratteri, un valore di 4-5 cifre e l'unità.
