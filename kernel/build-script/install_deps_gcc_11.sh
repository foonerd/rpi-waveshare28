#!/bin/bash
#
# Toolchain for building Raspberry Pi kernel modules.
#
# The compiler is not a free choice. The Raspberry Pi buildbot publishes the
# exact toolchain it used at every kernel commit, in the `uname_string` file
# in raspberrypi/rpi-firmware. For every 6.6 and 6.12 kernel checked, that is:
#
#   arm-linux-gnueabihf-gcc (Ubuntu 11.4.0-1ubuntu1~22.04) 11.4.0
#   GNU ld (GNU Binutils for Ubuntu) 2.38
#
# Why it matters: bcm2709_defconfig sets CONFIG_MODVERSIONS=y, so every
# exported symbol a module uses is CRC-checked against the running kernel.
# Because we rebuild the whole kernel to obtain Module.symvers, those CRCs
# come from our build. Kconfig evaluates its CC_HAS_* symbols against the
# actual compiler at configure time, so the same defconfig fed to a different
# GCC silently produces a different .config, different struct layouts and
# different CRCs. The module then fails to load with "disagrees about version
# of symbol module_layout".
#
# Note that the compiler does not appear in the vermagic string, so this does
# not present as a vermagic mismatch. It presents as a symbol CRC mismatch,
# which is easier to misdiagnose.
#
# download_build.sh verifies the installed compiler against uname_string
# before doing any work, so a wrong toolchain fails in seconds rather than
# after an hour of building something unloadable.

set -euo pipefail

sudo apt update
sudo apt -y install git bc bison flex libssl-dev make libc6-dev libncurses5-dev \
    crossbuild-essential-armhf crossbuild-essential-arm64

sudo apt -y install gcc-11-arm-linux-gnueabihf gcc-11-aarch64-linux-gnu \
    g++-11-arm-linux-gnueabihf g++-11-aarch64-linux-gnu

sudo update-alternatives --install /usr/bin/arm-linux-gnueabihf-gcc arm-linux-gnueabihf-gcc \
    /usr/bin/arm-linux-gnueabihf-gcc-11 11 \
    --slave /usr/bin/arm-linux-gnueabihf-g++ arm-linux-gnueabihf-g++ /usr/bin/arm-linux-gnueabihf-g++-11

sudo update-alternatives --install /usr/bin/aarch64-linux-gnu-gcc aarch64-linux-gnu-gcc \
    /usr/bin/aarch64-linux-gnu-gcc-11 11 \
    --slave /usr/bin/aarch64-linux-gnu-g++ aarch64-linux-gnu-g++ /usr/bin/aarch64-linux-gnu-g++-11

echo
echo "Installed:"
arm-linux-gnueabihf-gcc --version | head -n1
aarch64-linux-gnu-gcc --version | head -n1
echo
echo "If these are not 11.x, select them with:"
echo "  sudo update-alternatives --config arm-linux-gnueabihf-gcc"
echo "  sudo update-alternatives --config aarch64-linux-gnu-gcc"
