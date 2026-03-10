.PHONY: dev build list clean firmware firmware-clean

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

firmware-clean:
	$(MAKE) -C firmware clean
