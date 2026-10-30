.PHONY: heltec-v3-build heltec-v3-run heltec-v3-flash heltec-v3-bins heltec-v3-full-bin heltec-v3-upgrade-bin heltec-v4-build heltec-v4-run heltec-v4-flash heltec-v4-bins heltec-v4-full-bin heltec-v4-upgrade-bin heltec-wsl3-build heltec-wsl3-run heltec-wsl3-flash heltec-wsl3-bins heltec-wsl3-full-bin heltec-wsl3-upgrade-bin clean-dist print-version

HELTEC_PARTITIONS := partitions.csv
HELTEC_V3_CHIP := esp32s3
HELTEC_V3_FLASH_SIZE := 8mb
HELTEC_V3_FLASH_MODE := dio
HELTEC_V3_FLASH_FREQ := 40mhz
HELTEC_V3_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-heltec_v3
HELTEC_V4_CHIP := esp32s3
HELTEC_V4_FLASH_SIZE := 16mb
HELTEC_V4_FLASH_MODE := dio
HELTEC_V4_FLASH_FREQ := 40mhz
HELTEC_V4_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-heltec_v4
HELTEC_WSL3_CHIP := esp32s3
HELTEC_WSL3_FLASH_SIZE := 8mb
HELTEC_WSL3_FLASH_MODE := dio
HELTEC_WSL3_FLASH_FREQ := 40mhz
HELTEC_WSL3_ELF := target/xtensa-esp32s3-none-elf/release/mcrs-heltec_wsl3
DIST_DIR := dist
MQTT ?= 0
# One optional profile file, applied after defaults.conf.
# Bare filenames are resolved in profiles/; paths are relative to the workspace.
PROFILE ?=
# Cargo reserves PROFILE for debug/release.
export MCRS_PROFILE := $(PROFILE)
ifneq ($(strip $(PROFILE)),)
empty :=
space := $(empty) $(empty)
PROFILE_SUFFIX := -$(basename $(notdir $(subst $(space),_,$(PROFILE))))
endif
# Command-line VERSION overrides this for reproducible packaging in CI.
VERSION := $(shell git describe --tags --dirty --always --abbrev=8 --match 'v[0-9]*' 2>/dev/null | tr '/' '-')
ifeq ($(strip $(VERSION)),)
VERSION := unknown
endif

ifeq ($(MQTT),1)
MQTT_FEATURE := --features mqtt
MQTT_SUFFIX := -mqtt
endif

heltec-v3-build:
	cd firmware/esp32s3 && cargo +esp build --release --bin mcrs-heltec_v3 $(MQTT_FEATURE)

heltec-v3-run heltec-v3-flash:
	cd firmware/esp32s3 && cargo +esp run --release --bin mcrs-heltec_v3 $(MQTT_FEATURE)

heltec-v3-bins: heltec-v3-full-bin heltec-v3-upgrade-bin

heltec-v3-full-bin: heltec-v3-build | $(DIST_DIR)
	cd firmware/esp32s3 && espflash save-image \
		--chip $(HELTEC_V3_CHIP) \
		--flash-size $(HELTEC_V3_FLASH_SIZE) \
		--flash-mode $(HELTEC_V3_FLASH_MODE) \
		--flash-freq $(HELTEC_V3_FLASH_FREQ) \
		--partition-table $(HELTEC_PARTITIONS) \
		--merge \
		$(HELTEC_V3_ELF) \
		../../$(DIST_DIR)/mcrs-heltec-v3$(PROFILE_SUFFIX)$(MQTT_SUFFIX)-$(VERSION)-merged.bin

heltec-v3-upgrade-bin: heltec-v3-build | $(DIST_DIR)
	cd firmware/esp32s3 && espflash save-image \
		--chip $(HELTEC_V3_CHIP) \
		--flash-size $(HELTEC_V3_FLASH_SIZE) \
		--flash-mode $(HELTEC_V3_FLASH_MODE) \
		--flash-freq $(HELTEC_V3_FLASH_FREQ) \
		--partition-table $(HELTEC_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_V3_ELF) \
		../../$(DIST_DIR)/mcrs-heltec-v3$(PROFILE_SUFFIX)$(MQTT_SUFFIX)-$(VERSION)-upgrade.bin

heltec-v4-build:
	cd firmware/esp32s3 && cargo +esp build --release --bin mcrs-heltec_v4 $(MQTT_FEATURE)

heltec-v4-run heltec-v4-flash:
	cd firmware/esp32s3 && cargo +esp run --release --bin mcrs-heltec_v4 $(MQTT_FEATURE)

heltec-v4-bins: heltec-v4-full-bin heltec-v4-upgrade-bin

heltec-v4-full-bin: heltec-v4-build | $(DIST_DIR)
	cd firmware/esp32s3 && espflash save-image \
		--chip $(HELTEC_V4_CHIP) \
		--flash-size $(HELTEC_V4_FLASH_SIZE) \
		--flash-mode $(HELTEC_V4_FLASH_MODE) \
		--flash-freq $(HELTEC_V4_FLASH_FREQ) \
		--partition-table $(HELTEC_PARTITIONS) \
		--merge \
		$(HELTEC_V4_ELF) \
		../../$(DIST_DIR)/mcrs-heltec-v4$(PROFILE_SUFFIX)$(MQTT_SUFFIX)-$(VERSION)-merged.bin

heltec-v4-upgrade-bin: heltec-v4-build | $(DIST_DIR)
	cd firmware/esp32s3 && espflash save-image \
		--chip $(HELTEC_V4_CHIP) \
		--flash-size $(HELTEC_V4_FLASH_SIZE) \
		--flash-mode $(HELTEC_V4_FLASH_MODE) \
		--flash-freq $(HELTEC_V4_FLASH_FREQ) \
		--partition-table $(HELTEC_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_V4_ELF) \
		../../$(DIST_DIR)/mcrs-heltec-v4$(PROFILE_SUFFIX)$(MQTT_SUFFIX)-$(VERSION)-upgrade.bin

heltec-wsl3-build:
	cd firmware/esp32s3 && cargo +esp build --release --bin mcrs-heltec_wsl3 $(MQTT_FEATURE)

heltec-wsl3-run heltec-wsl3-flash:
	cd firmware/esp32s3 && cargo +esp run --release --bin mcrs-heltec_wsl3 $(MQTT_FEATURE)

heltec-wsl3-bins: heltec-wsl3-full-bin heltec-wsl3-upgrade-bin

heltec-wsl3-full-bin: heltec-wsl3-build | $(DIST_DIR)
	cd firmware/esp32s3 && espflash save-image \
		--chip $(HELTEC_WSL3_CHIP) \
		--flash-size $(HELTEC_WSL3_FLASH_SIZE) \
		--flash-mode $(HELTEC_WSL3_FLASH_MODE) \
		--flash-freq $(HELTEC_WSL3_FLASH_FREQ) \
		--partition-table $(HELTEC_PARTITIONS) \
		--merge \
		$(HELTEC_WSL3_ELF) \
		../../$(DIST_DIR)/mcrs-heltec-wsl3$(PROFILE_SUFFIX)$(MQTT_SUFFIX)-$(VERSION)-merged.bin

heltec-wsl3-upgrade-bin: heltec-wsl3-build | $(DIST_DIR)
	cd firmware/esp32s3 && espflash save-image \
		--chip $(HELTEC_WSL3_CHIP) \
		--flash-size $(HELTEC_WSL3_FLASH_SIZE) \
		--flash-mode $(HELTEC_WSL3_FLASH_MODE) \
		--flash-freq $(HELTEC_WSL3_FLASH_FREQ) \
		--partition-table $(HELTEC_PARTITIONS) \
		--target-app-partition ota_0 \
		$(HELTEC_WSL3_ELF) \
		../../$(DIST_DIR)/mcrs-heltec-wsl3$(PROFILE_SUFFIX)$(MQTT_SUFFIX)-$(VERSION)-upgrade.bin

$(DIST_DIR):
	mkdir -p $(DIST_DIR)

clean-dist:
	rm -rf $(DIST_DIR)

print-version:
	@printf '%s\n' '$(VERSION)'
