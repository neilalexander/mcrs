.PHONY: heltec-v3-build heltec-v3-run heltec-v3-flash heltec-v3-bins heltec-v3-full-bin heltec-v3-upgrade-bin heltec-v4-build heltec-v4-run heltec-v4-flash heltec-v4-bins heltec-v4-full-bin heltec-v4-upgrade-bin heltec-wsl3-build heltec-wsl3-run heltec-wsl3-flash heltec-wsl3-bins heltec-wsl3-full-bin heltec-wsl3-upgrade-bin clean-dist

HELTEC_V3_CHIP := esp32s3
HELTEC_V3_FLASH_SIZE := 8mb
HELTEC_V3_FLASH_MODE := dio
HELTEC_V3_FLASH_FREQ := 40mhz
HELTEC_V3_PARTITIONS := firmware/partitions_heltec_v3.csv
HELTEC_V3_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-firmware
HELTEC_V4_CHIP := esp32s3
HELTEC_V4_FLASH_SIZE := 16mb
HELTEC_V4_FLASH_MODE := dio
HELTEC_V4_FLASH_FREQ := 40mhz
HELTEC_V4_PARTITIONS := firmware/partitions_heltec_v4.csv
HELTEC_V4_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-firmware
HELTEC_WSL3_CHIP := esp32s3
HELTEC_WSL3_FLASH_SIZE := 8mb
HELTEC_WSL3_FLASH_MODE := dio
HELTEC_WSL3_FLASH_FREQ := 40mhz
HELTEC_WSL3_PARTITIONS := firmware/partitions_heltec_v3.csv
HELTEC_WSL3_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-firmware
DIST_DIR := dist
MQTT ?= 0

ifeq ($(MQTT),1)
MQTT_FEATURE := --features mqtt
MQTT_SUFFIX := -mqtt
endif

heltec-v3-build:
	cargo +esp build-heltec-v3 $(MQTT_FEATURE)

heltec-v3-run heltec-v3-flash:
	cargo +esp run-heltec-v3 $(MQTT_FEATURE)

heltec-v3-bins: heltec-v3-full-bin heltec-v3-upgrade-bin

heltec-v3-full-bin: heltec-v3-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V3_CHIP) \
		--flash-size $(HELTEC_V3_FLASH_SIZE) \
		--flash-mode $(HELTEC_V3_FLASH_MODE) \
		--flash-freq $(HELTEC_V3_FLASH_FREQ) \
		--partition-table $(HELTEC_V3_PARTITIONS) \
		--merge \
		$(HELTEC_V3_ELF) \
		$(DIST_DIR)/mcrs-heltec-v3$(MQTT_SUFFIX)-full.bin

heltec-v3-upgrade-bin: heltec-v3-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V3_CHIP) \
		--flash-size $(HELTEC_V3_FLASH_SIZE) \
		--flash-mode $(HELTEC_V3_FLASH_MODE) \
		--flash-freq $(HELTEC_V3_FLASH_FREQ) \
		--partition-table $(HELTEC_V3_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_V3_ELF) \
		$(DIST_DIR)/mcrs-heltec-v3$(MQTT_SUFFIX)-upgrade.bin

heltec-v4-build:
	cargo +esp build-heltec-v4 $(MQTT_FEATURE)

heltec-v4-run heltec-v4-flash:
	cargo +esp run-heltec-v4 $(MQTT_FEATURE)

heltec-v4-bins: heltec-v4-full-bin heltec-v4-upgrade-bin

heltec-v4-full-bin: heltec-v4-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V4_CHIP) \
		--flash-size $(HELTEC_V4_FLASH_SIZE) \
		--flash-mode $(HELTEC_V4_FLASH_MODE) \
		--flash-freq $(HELTEC_V4_FLASH_FREQ) \
		--partition-table $(HELTEC_V4_PARTITIONS) \
		--merge \
		$(HELTEC_V4_ELF) \
		$(DIST_DIR)/mcrs-heltec-v4$(MQTT_SUFFIX)-full.bin

heltec-v4-upgrade-bin: heltec-v4-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_V4_CHIP) \
		--flash-size $(HELTEC_V4_FLASH_SIZE) \
		--flash-mode $(HELTEC_V4_FLASH_MODE) \
		--flash-freq $(HELTEC_V4_FLASH_FREQ) \
		--partition-table $(HELTEC_V4_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_V4_ELF) \
		$(DIST_DIR)/mcrs-heltec-v4$(MQTT_SUFFIX)-upgrade.bin

heltec-wsl3-build:
	cargo +esp build-heltec-wsl3 $(MQTT_FEATURE)

heltec-wsl3-run heltec-wsl3-flash:
	cargo +esp run-heltec-wsl3 $(MQTT_FEATURE)

heltec-wsl3-bins: heltec-wsl3-full-bin heltec-wsl3-upgrade-bin

heltec-wsl3-full-bin: heltec-wsl3-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_WSL3_CHIP) \
		--flash-size $(HELTEC_WSL3_FLASH_SIZE) \
		--flash-mode $(HELTEC_WSL3_FLASH_MODE) \
		--flash-freq $(HELTEC_WSL3_FLASH_FREQ) \
		--partition-table $(HELTEC_WSL3_PARTITIONS) \
		--merge \
		$(HELTEC_WSL3_ELF) \
		$(DIST_DIR)/mcrs-heltec-wsl3$(MQTT_SUFFIX)-full.bin

heltec-wsl3-upgrade-bin: heltec-wsl3-build | $(DIST_DIR)
	espflash save-image \
		--chip $(HELTEC_WSL3_CHIP) \
		--flash-size $(HELTEC_WSL3_FLASH_SIZE) \
		--flash-mode $(HELTEC_WSL3_FLASH_MODE) \
		--flash-freq $(HELTEC_WSL3_FLASH_FREQ) \
		--partition-table $(HELTEC_WSL3_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_WSL3_ELF) \
		$(DIST_DIR)/mcrs-heltec-wsl3$(MQTT_SUFFIX)-upgrade.bin

$(DIST_DIR):
	mkdir -p $(DIST_DIR)

clean-dist:
	rm -rf $(DIST_DIR)
