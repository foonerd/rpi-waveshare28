# waveshare28-panel cross builds.
#
# Default target set is musl, statically linked, runs on Buster through Trixie.
# The gnu set is provided for parity with sister repos; those binaries need
# glibc 2.31 or newer and will not run on Volumio 3.

CRATE      := waveshare28-panel
MUSL       := armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl
GNU        := armv7-unknown-linux-gnueabihf aarch64-unknown-linux-gnu
DIST       := dist

.PHONY: all musl gnu check fmt clippy clean dist

all: musl

musl:
	@for t in $(MUSL); do \
		echo "=== $$t"; \
		cross build --release --target $$t || exit 1; \
	done

gnu:
	@for t in $(GNU); do \
		echo "=== $$t"; \
		cross build --release --target $$t || exit 1; \
	done

check:
	cargo check --all-targets

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets -- -D warnings

dist: musl
	@mkdir -p $(DIST)
	@for t in $(MUSL); do \
		cp target/$$t/release/$(CRATE) $(DIST)/$(CRATE)-$$t; \
	done
	@cd $(DIST) && sha256sum $(CRATE)-* > SHA256SUMS
	@ls -l $(DIST)

clean:
	cargo clean
	rm -rf $(DIST)
