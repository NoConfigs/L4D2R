# Android toolchain installed by the APK build setup (see tools/build-apk.ps1).
D="$LOCALAPPDATA/l4d2r-android"
export JAVA_HOME="$(cygpath -w "$D/jdk-17.0.21.0.2+1")"
export ANDROID_HOME="$(cygpath -w "$D/sdk")"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_ROOT="$ANDROID_HOME\ndk\27.2.12479018"
export ANDROID_NDK_HOME="$ANDROID_NDK_ROOT"
