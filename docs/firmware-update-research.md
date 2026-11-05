# HLK-LD2402 firmware update protocol: research notes

Date: 2026-11-05. Time-boxed: a handful of web searches, no fetched binaries, nothing executed. Web content treated as untrusted.

## Question
Is the LD2402 firmware update (OTA / bootloader) protocol publicly documented, so the app could implement it?

## Sources checked
- Local: HLK-LD2402 user manual, section 4.2.5 "Update the firmware" (describes the vendor tool: "Get Firmware Information", select .bin, "Download"; no wire protocol).
- https://manuals.plus/ae/1005008053355185 (manual copy: factory-burned firmware, HLK-LD2402_Tool, 115200 baud, tool and third-party serial tool cannot be used at once)
- https://hackaday.io/project/204353-a-comparison-of-three-radar-modules (cites manual V1.08; covers version/serial commands only)
- https://community.home-assistant.io/t/hlk-ld2450-initial-experiments-to-connect-to-homeassistant/578878 (LD2450: Bluetooth upgrade via vendor app; a report of a bricked sensor after an update)
- https://www.tinytronics.nl/product_files/006002_HLK-LD2410S%20serial%20communication%20protocol-V1.00.pdf (sibling LD2410S protocol; different model)
- https://github.com/tsunglung/esphome-ld2450, https://github.com/bertrik/hlk-ld2410, https://github.com/Fiooodooor/HLK-LD245X (other models; no firmware flashing)
- GitHub / ESPHome searches for "ld2402": no component or library found.

## Findings
- No public document, repository or forum post describes the LD2402 update protocol (framing, bootloader entry, image format, checksums).
- The manual only describes the vendor Windows tool's UI. It notes a "Get Firmware Information" step is mandatory before download, implying a handshake/ID check we cannot reproduce.
- Related Hi-Link modules use separate, undocumented upgrade paths (Bluetooth for LD2450); nothing transfers to the LD2402.
- One forum report (LD2450, a different model) describes a sensor left unresponsive after an update. This is a single anecdote, not evidence about the LD2402, but it is a reason to avoid untested flashing code.

## Conclusion
The protocol is not publicly available. No flashing code is implemented. The Firmware page ships in an "unavailable" state (current version and feature table only; Update disabled).

## Next steps if pursued
1. Ask Hi-Link support/sales for the protocol document and a sample .bin (no source confirms they provide it).
2. Otherwise capture the vendor tool's serial traffic on Windows (virtual COM pair / USB analyser) during an update on a sacrificial module.
3. Write a separate spec and plan, reviewed by the user, covering recovery behaviour, image validation and a dry-run mode before any write code.
