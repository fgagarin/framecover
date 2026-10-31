#!/usr/bin/env bash
# Build static FFmpeg libraries (libav*) plus the ffmpeg/ffprobe front ends as
# a linkable library (libfftools.a: `ffmpeg_main` / `ffprobe_main`) for the
# in-process `libav` backend of framecover-core.
#
# Usage (inside the image from scripts/android/Dockerfile):
#   scripts/android/build-ffmpeg.sh <platform>...
# Platforms: android-arm64-v8a, android-x86_64, linux-x64.
#
# Output: vendor/ffmpeg/libav/<platform>/{include,lib,licenses} and libs.txt,
# the `pkg-config --static --libs` line the Rust build script links with.
# Sources are pinned in sources.env; builds happen under target/libav-build.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
WORK=${WORK:-$ROOT/target/libav-build}
OUT_ROOT=$ROOT/vendor/ffmpeg/libav
API=${ANDROID_API:-26}
JOBS=$(nproc)

# shellcheck source=sources.env
source "$SCRIPT_DIR/sources.env"

# Components the core needs (see probe.rs, frames.rs, frame_stream.rs,
# thumbnail_writer.rs, converter.rs); everything else is left out.
# Raw demuxers (mpegvideo, m4v, h264, hevc) also identify codecs inside
# program and transport streams.
DEMUXERS=mov,matroska,avi,flv,concat,image2,image_png_pipe,image_jpeg_pipe,mpegts,mpegps,asf,ogg,wav,mp3,aac,h264,hevc,mpegvideo,m4v
DECODERS=h264,hevc,libdav1d,vp8,vp9,mpeg4,mpeg1video,mpeg2video,mjpeg,png,bmp,gif,prores,ffv1,h263,msmpeg4v3,wmv2,wmv3,vc1,theora,aac,mp1,mp2,mp2float,mp3,mp3float,ac3,eac3,opus,vorbis,flac,alac,wmav2,pcm_s16le,pcm_s16be,pcm_s24le,pcm_f32le
ENCODERS=libx264,libx265,libaom_av1,aac,png,ppm,rawvideo
MUXERS=mp4,mov,matroska,avi,image2,image2pipe,rawvideo,null
PARSERS=h264,hevc,av1,png,mjpeg,mpeg4video,mpegvideo,vp8,vp9,aac,ac3,mpegaudio,opus,vorbis,flac
BSFS=setts,noise,null,h264_mp4toannexb,hevc_mp4toannexb,aac_adtstoasc,extract_extradata,vp9_superframe,vp9_superframe_split,av1_frame_split
FILTERS=null,anull,copy,format,aformat,aresample,scale,zscale,tonemap,setsar,trim,atrim,setpts,asetpts,concat,pad,showinfo
PROTOCOLS=file,pipe,fd

log() { printf '\n==> %s\n' "$*"; }

# fetch NAME: shallow-clone <NAME>_REF and check it is <NAME>_COMMIT.
fetch() {
    local name=$1 url ref commit dir
    url=${name}_URL ref=${name}_REF commit=${name}_COMMIT
    url=${!url} ref=${!ref} commit=${!commit}
    dir=$WORK/src/${name,,}
    if [[ -d $dir ]] && [[ $(git -C "$dir" rev-parse HEAD) == "$commit" ]]; then
        return
    fi
    log "fetch $name $ref"
    rm -rf "$dir"
    git clone -q --depth 1 --branch "$ref" --recurse-submodules --shallow-submodules "$url" "$dir"
    local got
    got=$(git -C "$dir" rev-parse HEAD)
    if [[ $got != "$commit" ]]; then
        echo "error: $name $ref is $got, expected $commit (update sources.env)" >&2
        exit 1
    fi
}

# Toolchain variables for one platform.
setup() {
    PLATFORM=$1
    PREFIX=$WORK/$PLATFORM/prefix
    BUILD=$WORK/$PLATFORM/build
    mkdir -p "$PREFIX" "$BUILD"
    export PKG_CONFIG_LIBDIR=$PREFIX/lib/pkgconfig
    unset PKG_CONFIG_PATH
    case $PLATFORM in
        android-*)
            local tc=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64
            case $PLATFORM in
                android-arm64-v8a) TRIPLE=aarch64-linux-android ARCH=aarch64 ABI=arm64-v8a ;;
                android-x86_64) TRIPLE=x86_64-linux-android ARCH=x86_64 ABI=x86_64 ;;
                *) echo "unknown platform $PLATFORM" >&2; exit 1 ;;
            esac
            CC=$tc/bin/${TRIPLE}${API}-clang
            CXX=$tc/bin/${TRIPLE}${API}-clang++
            AR=$tc/bin/llvm-ar NM=$tc/bin/llvm-nm RANLIB=$tc/bin/llvm-ranlib
            STRIP=$tc/bin/llvm-strip OBJCOPY=$tc/bin/llvm-objcopy LD_R="$tc/bin/ld.lld -r"
            SYSROOT=$tc/sysroot
            CROSS_PREFIX=$tc/bin/llvm-
            CXXLIBS="-lc++_static -lc++abi"
            CMAKE_ARGS=(
                -DCMAKE_TOOLCHAIN_FILE="$ANDROID_NDK_HOME/build/cmake/android.toolchain.cmake"
                -DANDROID_ABI="$ABI" -DANDROID_PLATFORM="android-$API" -DANDROID_STL=c++_static
            )
            ;;
        linux-x64)
            TRIPLE="" ARCH=x86_64
            CC=gcc CXX=g++ AR=ar NM=nm RANLIB=ranlib STRIP=strip OBJCOPY=objcopy LD_R="ld -r"
            SYSROOT="" CROSS_PREFIX=""
            CXXLIBS="-lstdc++"
            CMAKE_ARGS=()
            ;;
        *) echo "unknown platform $PLATFORM" >&2; exit 1 ;;
    esac
    export CC CXX AR NM RANLIB STRIP
    export CFLAGS="-O2 -fPIC" CXXFLAGS="-O2 -fPIC"
    CMAKE_ARGS+=(
        -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX"
        -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_POSITION_INDEPENDENT_CODE=ON
        -DBUILD_SHARED_LIBS=OFF -DCMAKE_POLICY_VERSION_MINIMUM=3.5
    )
}

host_flag() { [[ -n $TRIPLE ]] && echo "--host=$TRIPLE"; true; }

# Fresh per-platform copy of a source tree (in-tree builds can't be shared).
work_copy() {
    rm -rf "$BUILD/$1"
    cp -a "$WORK/src/$1" "$BUILD/$1"
    cd "$BUILD/$1"
}

build_zimg() {
    log "$PLATFORM: zimg"
    work_copy zimg
    ./autogen.sh >/dev/null
    # shellcheck disable=SC2046
    ./configure --prefix="$PREFIX" $(host_flag) --disable-shared --enable-static >/dev/null
    make -j"$JOBS" >/dev/null
    make install >/dev/null
}

build_x264() {
    log "$PLATFORM: x264"
    work_copy x264
    local args=(--prefix="$PREFIX" --enable-static --enable-pic --disable-cli)
    if [[ -n $TRIPLE ]]; then
        args+=(--host="$TRIPLE" --cross-prefix="$CROSS_PREFIX" --sysroot="$SYSROOT")
    fi
    ./configure "${args[@]}" >/dev/null
    make -j"$JOBS" >/dev/null
    make install >/dev/null
}

build_x265() {
    log "$PLATFORM: x265"
    work_copy x265
    # x265 sets m_firstPts / m_bframeDelayTime only once enough frames for
    # the B-frame delay arrive, so the dts of a one-frame encode (the preview
    # frame) comes from uninitialised memory. A fresh ffmpeg process usually
    # reads zeros; in-process encodes reuse the heap and get garbage, which
    # the mp4 muxer rejects ("pts/dts pair unsupported").
    grep -q '^    m_numDelayedPic = 0;$' source/encoder/encoder.cpp
    sed -i 's/^    m_numDelayedPic = 0;$/&\n    m_firstPts = 0;\n    m_bframeDelayTime = 0;/' \
        source/encoder/encoder.cpp
    # x265's aarch64 assembly does not build with the NDK's clang; x86_64
    # keeps its (nasm) assembly.
    local asm=ON
    [[ $PLATFORM == android-arm64-v8a ]] && asm=OFF
    # Multilib, like x265's own multilib.sh: a 10-bit build (C API hidden)
    # linked into the 8-bit library, so libx265 encodes Main and Main 10.
    cmake -S source -B out10 "${CMAKE_ARGS[@]}" -DENABLE_SHARED=OFF -DENABLE_CLI=OFF \
        -DENABLE_ASSEMBLY="$asm" -DHIGH_BIT_DEPTH=ON -DEXPORT_C_API=OFF >/dev/null
    cmake --build out10 >/dev/null
    cp out10/libx265.a out10/libx265_main10.a
    cmake -S source -B out "${CMAKE_ARGS[@]}" -DENABLE_SHARED=OFF -DENABLE_CLI=OFF \
        -DENABLE_ASSEMBLY="$asm" -DLINKED_10BIT=ON \
        -DEXTRA_LIB=x265_main10.a -DEXTRA_LINK_FLAGS="-L$PWD/out10" >/dev/null
    cmake --build out >/dev/null
    cmake --install out >/dev/null
    rm -f "$PREFIX/lib/libx265.a"
    printf 'CREATE %s\nADDLIB out/libx265.a\nADDLIB out10/libx265_main10.a\nSAVE\nEND\n' \
        "$PREFIX/lib/libx265.a" | $AR -M
    # x265.pc names the host C++ runtime; use this platform's instead.
    # CMake's implicit NDK libraries come out as `-l-l:libunwind.a`; clang
    # adds the unwinder itself.
    sed -i -E "s/-lstdc\+\+|-lc\+\+//; s/-lgcc_s|-lgcc//g; s/-l-l:[^ ]+//g; s/^(Libs\.private:.*)$/\1 $CXXLIBS/" \
        "$PREFIX/lib/pkgconfig/x265.pc"
    if [[ $PLATFORM == android-* ]]; then
        sed -i -E 's/ -l(pthread|rt)\b//g' "$PREFIX/lib/pkgconfig/x265.pc"
    fi
}

build_aom() {
    log "$PLATFORM: libaom"
    work_copy aom
    local cpu=$ARCH
    [[ $ARCH == aarch64 ]] && cpu=arm64
    cmake -S . -B out "${CMAKE_ARGS[@]}" -DAOM_TARGET_CPU="$cpu" \
        -DENABLE_DOCS=0 -DENABLE_EXAMPLES=0 -DENABLE_TESTS=0 -DENABLE_TOOLS=0 \
        -DCONFIG_AV1_DECODER=0 >/dev/null
    cmake --build out >/dev/null
    cmake --install out >/dev/null
}

build_dav1d() {
    log "$PLATFORM: dav1d"
    work_copy dav1d
    local cross=()
    if [[ -n $TRIPLE ]]; then
        cat >cross.ini <<EOF
[binaries]
c = '$CC'
cpp = '$CXX'
ar = '$AR'
strip = '$STRIP'
pkg-config = 'pkg-config'

[host_machine]
system = 'android'
cpu_family = '$ARCH'
cpu = '$ARCH'
endian = 'little'
EOF
        cross=(--cross-file cross.ini)
    fi
    meson setup out "${cross[@]}" --prefix="$PREFIX" --libdir=lib --buildtype=release \
        --default-library=static -Denable_tools=false -Denable_tests=false >/dev/null
    ninja -C out >/dev/null
    ninja -C out install >/dev/null
}

build_ffmpeg() {
    log "$PLATFORM: ffmpeg"
    work_copy ffmpeg
    local args=(
        --prefix="$PREFIX" --pkg-config=pkg-config --pkg-config-flags=--static
        --enable-gpl --disable-shared --enable-static --enable-pic
        --disable-doc --disable-debug --disable-autodetect --enable-pthreads
        # zlib: the png codecs and compressed mov / matroska headers need it.
        --enable-zlib
        --disable-everything --disable-ffplay --enable-ffmpeg --enable-ffprobe
        --enable-avcodec --enable-avformat --enable-avfilter --enable-swscale --enable-swresample
        --enable-libx264 --enable-libx265 --enable-libaom --enable-libdav1d --enable-libzimg
        --enable-demuxer="$DEMUXERS" --enable-decoder="$DECODERS" --enable-encoder="$ENCODERS"
        --enable-muxer="$MUXERS" --enable-parser="$PARSERS" --enable-bsf="$BSFS"
        --enable-filter="$FILTERS" --enable-protocol="$PROTOCOLS"
        --extra-cflags="-I$PREFIX/include -fPIC" --extra-ldflags="-L$PREFIX/lib"
        --extra-libs="$CXXLIBS -lm"
    )
    if [[ $PLATFORM == android-* ]]; then
        args+=(
            --target-os=android --arch="$ARCH" --enable-cross-compile --sysroot="$SYSROOT"
            --cc="$CC" --cxx="$CXX" --ar="$AR" --nm="$NM" --ranlib="$RANLIB" --strip="$STRIP"
        )
        [[ $ARCH == aarch64 ]] && args+=(--cpu=armv8-a)
    fi
    ./configure "${args[@]}" >configure.out || { tail -40 ffbuild/config.log; exit 1; }
    verify_components
    make -j"$JOBS" >/dev/null
    make install >/dev/null
    package_fftools
}

# configure drops a requested component whose dependency is missing (png
# without zlib) without failing; check every one made it in.
verify_components() {
    local kind list name missing=()
    for kind in DEMUXER DECODER ENCODER MUXER PARSER BSF FILTER PROTOCOL; do
        list=${kind}S
        list=${!list}
        for name in ${list//,/ }; do
            grep -q "^#define CONFIG_${name^^}_${kind} 1" config_components.h || missing+=("$name ($kind)")
        done
    done
    if ((${#missing[@]})); then
        echo "error: configure disabled: ${missing[*]}" >&2
        exit 1
    fi
}

# Link each front end's objects into one relocatable object whose only
# global definition is `<tool>_main` (renamed from `main`), so ffmpeg and
# ffprobe (which both define cmdutils globals) can live in one library.
# Their writable data moves to fc_<tool>_data / fc_<tool>_bss, so the
# linker's __start_/__stop_ symbols let fc_shim.c reset it between runs.
package_fftools() {
    local link tool objs sections renames sec
    rm -f ffmpeg_g ffprobe_g
    link=$(make V=1 ffmpeg_g ffprobe_g)
    for tool in ffmpeg ffprobe; do
        objs=$(grep -- "-o ${tool}_g" <<<"$link" | tr ' ' '\n' | grep -E '^fftools/.*\.o$')
        # shellcheck disable=SC2086
        $LD_R -o "${tool}_tool.o" $objs
        $OBJCOPY --redefine-sym main="${tool}_main" "${tool}_tool.o"
        $OBJCOPY --keep-global-symbol="${tool}_main" "${tool}_tool.o"
        sections=$(readelf -SW "${tool}_tool.o" | grep -oE '\] \.(data|bss)(\.[^ ]*)? ' |
            awk '{print $2}' | grep -v '^\.data\.rel\.ro' | sort -u)
        renames=()
        for sec in $sections; do
            case $sec in
                .bss*) renames+=(--rename-section "$sec=fc_${tool}_bss") ;;
                *) renames+=(--rename-section "$sec=fc_${tool}_data") ;;
            esac
        done
        $OBJCOPY "${renames[@]}" "${tool}_tool.o"
    done
    rm -f "$PREFIX/lib/libfftools.a"
    $AR rcs "$PREFIX/lib/libfftools.a" ffmpeg_tool.o ffprobe_tool.o
}

# step NAME lib: run build_<lib> unless this platform already built the
# pinned commit with this script (FORCE=1 rebuilds); sets SKIPPED=1 when it
# did nothing.
step() {
    local commit=${1}_COMMIT stamp=$PREFIX/.built-$2
    commit="${!commit} $(sha256sum "$SCRIPT_DIR/build-ffmpeg.sh" | cut -d' ' -f1)"
    if [[ -z ${FORCE:-} && -f $stamp && $(<"$stamp") == "$commit" ]]; then
        log "$PLATFORM: $2 (up to date)"
        SKIPPED=1
        return 0
    fi
    SKIPPED=0
    "build_$2"
    echo "$commit" >"$stamp"
}

publish() {
    local out=$OUT_ROOT/$PLATFORM
    log "$PLATFORM: publish to ${out#"$ROOT"/}"
    rm -rf "$out"
    mkdir -p "$out/lib" "$out/licenses"
    cp -a "$PREFIX/include" "$out/"
    cp "$PREFIX"/lib/*.a "$out/lib/"
    {
        echo -n "-lfftools "
        pkg-config --static --libs libavdevice libavfilter libavformat libavcodec libswscale libswresample libavutil
    } >"$out/libs.txt"
    cp "$WORK/src/ffmpeg/COPYING.GPLv3" "$out/licenses/FFmpeg-GPL-3.0.txt"
    cp "$WORK/src/x264/COPYING" "$out/licenses/x264-COPYING.txt"
    cp "$WORK/src/x265/COPYING" "$out/licenses/x265-COPYING.txt"
    cp "$WORK/src/aom/LICENSE" "$out/licenses/aom-LICENSE.txt"
    cp "$WORK/src/dav1d/COPYING" "$out/licenses/dav1d-COPYING.txt"
    cp "$WORK/src/zimg/COPYING" "$out/licenses/zimg-COPYING.txt"
}

main() {
    if [[ $# -eq 0 ]]; then
        echo "usage: $0 <android-arm64-v8a|android-x86_64|linux-x64>..." >&2
        exit 2
    fi
    for name in FFMPEG X264 X265 AOM DAV1D ZIMG; do
        fetch "$name"
    done
    for platform in "$@"; do
        setup "$platform"
        step ZIMG zimg
        step X264 x264
        step X265 x265
        step AOM aom
        step DAV1D dav1d
        step FFMPEG ffmpeg
        if [[ $SKIPPED == 1 ]]; then
            # Already built: only repackage the front ends.
            (cd "$BUILD/ffmpeg" && package_fftools)
        fi
        publish
    done
}

main "$@"
