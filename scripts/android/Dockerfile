# Build environment for the in-process FFmpeg libraries (`libav`) used by the
# Android app and by the desktop `libav` feature. See build-ffmpeg.sh.
#
#   docker build -t framecover-ffmpeg-build scripts/android
#   docker run --rm -v "$PWD:/src" -v framecover-libav-work:/work -e WORK=/work \
#       framecover-ffmpeg-build \
#       scripts/android/build-ffmpeg.sh android-arm64-v8a android-x86_64 linux-x64
#
# The named volume keeps sources and build trees off the (slow, on Windows)
# bind mount; only the finished libraries land in vendor/ffmpeg/libav/.

FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        autoconf automake build-essential ca-certificates cmake curl git \
        libtool meson nasm ninja-build pkg-config python3 unzip zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# Android NDK r29; SHA-1 from Google's repository2-3.xml (the published
# checksum for this archive).
ARG NDK_VERSION=r29
ARG NDK_SHA1=87e2bb7e9be5d6a1c6cdf5ec40dd4e0c6d07c30b
RUN curl -fsSL -o /tmp/ndk.zip \
        "https://dl.google.com/android/repository/android-ndk-${NDK_VERSION}-linux.zip" \
    && echo "${NDK_SHA1}  /tmp/ndk.zip" | sha1sum -c - \
    && unzip -q /tmp/ndk.zip -d /opt \
    && mv "/opt/android-ndk-${NDK_VERSION}" /opt/android-ndk \
    && rm /tmp/ndk.zip
ENV ANDROID_NDK_HOME=/opt/android-ndk

# Rust for building the app's Android library (cargo ndk); the toolchain
# matches the one the desktop builds use.
ARG RUST_VERSION=1.99.1
ARG CARGO_NDK_VERSION=4.1.2
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain "${RUST_VERSION}" \
        --target aarch64-linux-android --target x86_64-linux-android \
    && cargo install cargo-ndk --locked --version "${CARGO_NDK_VERSION}" \
    && rm -rf /opt/cargo/registry

# Android SDK (android.jar and build tools: Slint's Android backend compiles
# Java helpers against it, and Gradle builds the APK). Installing it accepts
# the Android SDK License Agreement. Checksum from repository2-3.xml.
ARG CMDLINE_TOOLS=16111833
ARG CMDLINE_TOOLS_SHA1=e025545c62a8e64c7559119566a569fb1dec5f60
ARG GRADLE_VERSION=8.10.2
ARG GRADLE_SHA256=31c55713e40233a8303827ceb42ca48a47267a0ad4bab9177123121e71524c26
RUN apt-get update && apt-get install -y --no-install-recommends openjdk-17-jdk-headless \
    && rm -rf /var/lib/apt/lists/*
ENV ANDROID_HOME=/opt/android-sdk
RUN curl -fsSL -o /tmp/tools.zip \
        "https://dl.google.com/android/repository/commandlinetools-linux-${CMDLINE_TOOLS}_latest.zip" \
    && echo "${CMDLINE_TOOLS_SHA1}  /tmp/tools.zip" | sha1sum -c - \
    && mkdir -p "$ANDROID_HOME/cmdline-tools" \
    && unzip -q /tmp/tools.zip -d "$ANDROID_HOME/cmdline-tools" \
    && mv "$ANDROID_HOME/cmdline-tools/cmdline-tools" "$ANDROID_HOME/cmdline-tools/latest" \
    && rm /tmp/tools.zip \
    && yes | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --licenses >/dev/null \
    && "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" "platforms;android-35" "build-tools;35.0.0" >/dev/null
ENV ANDROID_JAR=/opt/android-sdk/platforms/android-35/android.jar
RUN curl -fsSL -o /tmp/gradle.zip \
        "https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip" \
    && echo "${GRADLE_SHA256}  /tmp/gradle.zip" | sha256sum -c - \
    && unzip -q /tmp/gradle.zip -d /opt \
    && mv "/opt/gradle-${GRADLE_VERSION}" /opt/gradle \
    && rm /tmp/gradle.zip
ENV PATH=/opt/gradle/bin:$PATH

WORKDIR /src
