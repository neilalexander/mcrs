.PHONY: heltec-v3-build heltec-v3-flash heltec-v3-bins heltec-v3-merged-bin heltec-v3-ota-bin heltec-v4-build heltec-v4-flash heltec-v4-bins heltec-v4-merged-bin heltec-v4-ota-bin clean-dist

HELTEC_V3_CHIP := esp32s3
HELTEC_V3_FLASH_SIZE := 8mb
HELTEC_V3_FLASH_MODE := dio
HELTEC_V3_FLASH_FREQ := 40mhz
HELTEC_V3_PARTITIONS := firmware/partitions_heltec_v3.csv
HELTEC_V3_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-firmware
HELTEC_V4_CHIP := esp32s3
HELTEC_V4_FLASH_SIZE := 16mb
HELTEC_V4_FLASH_MODE := qio
HELTEC_V4_FLASH_FREQ := 80mhz
HELTEC_V4_PARTITIONS := firmware/partitions_heltec_v4.csv
HELTEC_V4_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-firmware
DIST_DIR := dist

heltec-v3-build:
	cargo +esp build-heltec-v3

heltec-v3-flash:
	cargo +esp run-heltec-v3

heltec-v3-bins: heltec-v3-merged-bin heltec-v3-ota-bin

heltec-v3-merged-bin: heltec-v3-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V3_CHIP) \
		--flash-size $(HELTEC_V3_FLASH_SIZE) \
		--flash-mode $(HELTEC_V3_FLASH_MODE) \
		--flash-freq $(HELTEC_V3_FLASH_FREQ) \
		--partition-table $(HELTEC_V3_PARTITIONS) \
		--merge \
		$(HELTEC_V3_ELF) \
		$(DIST_DIR)/mcrs-heltec-v3-merged.bin

heltec-v3-ota-bin: heltec-v3-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V3_CHIP) \
		--flash-size $(HELTEC_V3_FLASH_SIZE) \
		--flash-mode $(HELTEC_V3_FLASH_MODE) \
		--flash-freq $(HELTEC_V3_FLASH_FREQ) \
		--partition-table $(HELTEC_V3_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_V3_ELF) \
		$(DIST_DIR)/mcrs-heltec-v3-app.bin

heltec-v4-build:
	cargo +esp build-heltec-v4

heltec-v4-flash:
	cargo +esp run-heltec-v4

heltec-v4-bins: heltec-v4-merged-bin heltec-v4-ota-bin

heltec-v4-merged-bin: heltec-v4-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V4_CHIP) \
		--flash-size $(HELTEC_V4_FLASH_SIZE) \
		--flash-mode $(HELTEC_V4_FLASH_MODE) \
		--flash-freq $(HELTEC_V4_FLASH_FREQ) \
		--partition-table $(HELTEC_V4_PARTITIONS) \
		--merge \
		$(HELTEC_V4_ELF) \
		$(DIST_DIR)/mcrs-heltec-v4-merged.bin

heltec-v4-ota-bin: heltec-v4-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V4_CHIP) \
		--flash-size $(HELTEC_V4_FLASH_SIZE) \
		--flash-mode $(HELTEC_V4_FLASH_MODE) \
		--flash-freq $(HELTEC_V4_FLASH_FREQ) \
		--partition-table $(HELTEC_V4_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_V4_ELF) \
		$(DIST_DIR)/mcrs-heltec-v4-app.bin

$(DIST_DIR):
	mkdir -p $(DIST_DIR)

clean-dist:
	rm -rf $(DIST_DIR)
