# Top-level dispatch. The two halves of this repository are independent and
# have nothing in common but the hardware they target, so this file delegates
# rather than sharing anything between them.
#
#   kernel/   cross-compiles a kernel module and a device tree overlay
#   runtime/  cross-compiles a userspace Rust binary
#
# They are mutually exclusive at runtime on a given boot: loading a display
# overlay on spi0 cs0 removes /dev/spidev0.0, which the runtime needs. Building
# both is fine; running both is not.

.PHONY: help kernel runtime clean

help:
	@echo "targets:"
	@echo "  kernel     build the CST328 module and overlay (see kernel/README.md)"
	@echo "  runtime    build the userspace renderer (see runtime/README.md)"
	@echo "  clean      clean both"
	@echo ""
	@echo "each subtree can also be built directly:"
	@echo "  cd kernel/build-script && ./download_build.sh"
	@echo "  cd runtime && make"

kernel:
	cd kernel/build-script && ./download_build.sh

runtime:
	$(MAKE) -C runtime

clean:
	$(MAKE) -C runtime clean
	rm -rf kernel/linux-*/
