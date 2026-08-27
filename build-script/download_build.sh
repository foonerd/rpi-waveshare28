#!/bin/bash
#
# Build the Hynitron CST328 touch module and overlay for Raspberry Pi kernels.
#
# The Raspberry Pi kernel tree carries drivers/input/touchscreen/hynitron_cstxxx.c
# but CONFIG_TOUCHSCREEN_HYNITRON_CSTXXX is not enabled in any Pi defconfig on
# 6.1, 6.6 or 6.12. This script enables it, adds a CST328 overlay, and builds
# both for all four Pi kernel variants.
#
# Only needed for the kernel input device path. The userspace renderer in this
# repository reads the CST328 over i2cdev and needs none of this.
#
# Output: output/cst328-rpi-<version>.tar.gz, extract over / on the target.

set -e

CPU=4
KERNEL_VERSION="6.12.75"

case $KERNEL_VERSION in
    "6.12.75")
      KERNEL_COMMIT="98655d3ccedba33aeadd0e550229f1496c5bf6f9"
      SOURCES="6.12.zz"
      ;;
    "6.12.74")
      KERNEL_COMMIT="7a35bddc777d8992bdfe42f8e3d043582df2f5f8"
      SOURCES="6.12.zy"
      ;;
    "6.12.50")
      KERNEL_COMMIT="a22bb2f110bc8953523714ac58251f47ae4e2d2b"
      SOURCES="6.12.zx"
      ;;
    "6.12.47")
      KERNEL_COMMIT="6d1da66a7b1358c9cd324286239f37203b7ce25c"
      SOURCES="6.12.z"
      ;;
    *)
      echo "!!!  Unknown kernel version ${KERNEL_VERSION}  !!!"
      exit 1
      ;;
esac

CONFIG_LINE="CONFIG_TOUCHSCREEN_HYNITRON_CSTXXX=m"
MODULE_PATH="drivers/input/touchscreen/hynitron_cstxxx.ko"
OVERLAY_DIR="arch/arm/boot/dts/overlays"

# Add the config symbol, the overlay source and the overlay Makefile entry to
# an extracted kernel tree. Idempotent.
apply_custom() {
    local TREE="$1"

    echo "!!!  Applying custom changes to ${TREE}  !!!"

    for DEFCONFIG in \
        arch/arm/configs/bcmrpi_defconfig \
        arch/arm/configs/bcm2709_defconfig \
        arch/arm/configs/bcm2711_defconfig \
        arch/arm64/configs/bcm2711_defconfig \
        arch/arm64/configs/bcm2712_defconfig
    do
        if [ -f "${TREE}/${DEFCONFIG}" ]; then
            if ! grep -q "^${CONFIG_LINE}$" "${TREE}/${DEFCONFIG}"; then
                echo "${CONFIG_LINE}" >> "${TREE}/${DEFCONFIG}"
                echo "    added ${CONFIG_LINE} to ${DEFCONFIG}"
            fi
        fi
    done

    cp "../source_files/${SOURCES}/overlays/cst328-overlay.dts" \
       "${TREE}/${OVERLAY_DIR}/cst328-overlay.dts"

    if ! grep -q "cst328.dtbo" "${TREE}/${OVERLAY_DIR}/Makefile"; then
        sed -i 's/^\tcma\.dtbo \\$/\tcma.dtbo \\\n\tcst328.dtbo \\/' \
            "${TREE}/${OVERLAY_DIR}/Makefile"
        echo "    added cst328.dtbo to overlays Makefile"
    fi

    if ! grep -q "cst328.dtbo" "${TREE}/${OVERLAY_DIR}/Makefile"; then
        echo "!!!  FAILED to add cst328.dtbo to the overlays Makefile  !!!"
        echo "!!!  The anchor line changed upstream, fix apply_custom()  !!!"
        exit 1
    fi
}

echo "!!!  Build CST328 touch module for kernel ${KERNEL_VERSION}  !!!"

echo "!!!  Download kernel hash info  !!!"
wget -N https://raw.githubusercontent.com/raspberrypi/rpi-firmware/${KERNEL_COMMIT}/git_hash
GIT_HASH="$(cat git_hash)"
rm git_hash

echo "!!!  Download kernel source  !!!"
wget https://github.com/raspberrypi/linux/archive/${GIT_HASH}.tar.gz

echo "!!!  Extract kernel source  !!!"
rm -rf linux-${KERNEL_VERSION}+/
tar xzf ${GIT_HASH}.tar.gz
rm ${GIT_HASH}.tar.gz
mv linux-${GIT_HASH}/ linux-${KERNEL_VERSION}+/

echo "!!!  Create git repo  !!!"
cd linux-${KERNEL_VERSION}+/
git init
git add --all
git commit -q -m "extracted files"
cd ..

apply_custom "linux-${KERNEL_VERSION}+"

echo "!!!  Copy source files for other variants  !!!"
rm -rf linux-${KERNEL_VERSION}-v7+/
rm -rf linux-${KERNEL_VERSION}-v7l+/
rm -rf linux-${KERNEL_VERSION}-v8+/
cp -r linux-${KERNEL_VERSION}+/ linux-${KERNEL_VERSION}-v7+/
cp -r linux-${KERNEL_VERSION}+/ linux-${KERNEL_VERSION}-v7l+/
cp -r linux-${KERNEL_VERSION}+/ linux-${KERNEL_VERSION}-v8+/

echo "!!!  Build RPi0 kernel and modules  !!!"
cd linux-${KERNEL_VERSION}+/
KERNEL=kernel
make -j${CPU} ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- bcmrpi_defconfig
make -j${CPU} ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- zImage modules dtbs
cd ..
echo "!!!  RPi0 build done  !!!"
echo "-------------------------"

echo "!!!  Build RPi2 kernel and modules  !!!"
cd linux-${KERNEL_VERSION}-v7+/
KERNEL=kernel7
make -j${CPU} ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- bcm2709_defconfig
make -j${CPU} ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- zImage modules dtbs
cd ..
echo "!!!  RPi2 build done  !!!"
echo "-------------------------"

echo "!!!  Build RPi3/4 32-bit kernel and modules  !!!"
cd linux-${KERNEL_VERSION}-v7l+/
KERNEL=kernel7l
make -j${CPU} ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- bcm2711_defconfig
make -j${CPU} ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- zImage modules dtbs
cd ..
echo "!!!  RPi3/4 32-bit build done  !!!"
echo "-------------------------"

echo "!!!  Build RPi3/4/5 64-bit kernel and modules  !!!"
cd linux-${KERNEL_VERSION}-v8+/
KERNEL=kernel8
make -j${CPU} ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- bcm2711_defconfig
make -j${CPU} ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- Image modules dtbs
cd ..
echo "!!!  RPi3/4/5 64-bit build done  !!!"
echo "-------------------------"

echo "!!!  Verify build products  !!!"
for V in "+" "-v7+" "-v7l+" "-v8+"; do
    if [ ! -f "linux-${KERNEL_VERSION}${V}/${MODULE_PATH}" ]; then
        echo "!!!  MISSING: linux-${KERNEL_VERSION}${V}/${MODULE_PATH}  !!!"
        exit 1
    fi
done
if [ ! -f "linux-${KERNEL_VERSION}+/${OVERLAY_DIR}/cst328.dtbo" ]; then
    echo "!!!  MISSING: cst328.dtbo  !!!"
    exit 1
fi

echo "!!!  Compressing modules with XZ  !!!"
xz -f linux-${KERNEL_VERSION}+/${MODULE_PATH}
xz -f linux-${KERNEL_VERSION}-v7+/${MODULE_PATH}
xz -f linux-${KERNEL_VERSION}-v7l+/${MODULE_PATH}
xz -f linux-${KERNEL_VERSION}-v8+/${MODULE_PATH}

echo "!!!  Creating archive  !!!"
PKG="cst328-rpi-${KERNEL_VERSION}"
rm -rf ${PKG}/

mkdir -p ${PKG}/boot/overlays
for V in "+" "-v7+" "-v7l+" "-v8+"; do
    mkdir -p ${PKG}/lib/modules/${KERNEL_VERSION}${V}/kernel/drivers/input/touchscreen/
    cp linux-${KERNEL_VERSION}${V}/${MODULE_PATH}* \
       ${PKG}/lib/modules/${KERNEL_VERSION}${V}/kernel/drivers/input/touchscreen/
done

cp linux-${KERNEL_VERSION}+/${OVERLAY_DIR}/cst328.dtbo ${PKG}/boot/overlays/

tar -czf ${PKG}.tar.gz ${PKG}/ --owner=0 --group=0
md5sum ${PKG}.tar.gz > ${PKG}.md5sum.txt
sha1sum ${PKG}.tar.gz > ${PKG}.sha1sum.txt
rm -rf ${PKG}/
mkdir -p ../output
mv ${PKG}* ../output/

echo "!!!  Done  !!!"
