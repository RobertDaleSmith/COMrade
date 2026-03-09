.PHONY: dev build list clean

dev:
	$(MAKE) -C app dev

build:
	$(MAKE) -C app build

list:
	$(MAKE) -C app list

clean:
	$(MAKE) -C app clean
