.PHONY: dev build list

# Launch the Tauri GUI app
dev:
	cd crates/comrade-app && cargo tauri dev

# Build the Tauri GUI app (.app bundle)
build:
	cd crates/comrade-app && cargo tauri build

# List serial ports via CLI
list:
	cargo run --bin comrade -- --list
