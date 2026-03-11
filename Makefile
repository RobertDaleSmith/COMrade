.PHONY: dev build list clean firmware firmware-w firmware-clean

dev:
	$(MAKE) -C app dev

build:
	$(MAKE) -C app build

list:
	$(MAKE) -C app list

clean:
	$(MAKE) -C app clean

firmware:
	$(MAKE) -C firmware build

firmware-w:
	$(MAKE) -C firmware build-w

firmware-clean:
	$(MAKE) -C firmware clean
