#!/bin/bash
#
# Rebuild from already-extracted and already-patched kernel trees.
# Run download_build.sh first. Use this after editing the overlay source
# or the driver, having copied the change into the trees yourself.

set -e

CPU=4
KERNEL_VERSION="6.12.75"

MODULE_PATH="drivers/input/touchscreen/hynitron_cstxxx.ko"
OVERLAY_DIR="arch/arm/boot/dts/overlays"

echo "!!!  Rebuild CST328 touch module for kernel ${KERNEL_VERSION}  !!!"

for V in "+" "-v7+" "-v7l+" "-v8+"; do
    if [ ! -d "linux-${KERNEL_VERSION}${V}" ]; then
        echo "!!!  linux-${KERNEL_VERSION}${V} not found, run download_build.sh  !!!"
        exit 1
    fi
    rm -f "linux-${KERNEL_VERSION}${V}/${MODULE_PATH}.xz"
done

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
