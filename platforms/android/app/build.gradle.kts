plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.physshell.abbreviate"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.physshell.abbreviate"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // We ship the engine for arm64 (CI builds that ABI); add more as needed.
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    // Hand-written shell under src/main/java; the UniFFI-generated binding is
    // dropped into src/uniffi/kotlin by `gen-bindings.sh` (git-ignored), and the
    // engine .so into src/main/jniLibs by cargo-ndk (git-ignored).
    sourceSets["main"].java.srcDirs("src/main/java", "src/uniffi/kotlin")
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")

    buildTypes {
        getByName("debug") {
            isMinifyEnabled = false
        }
    }
}

dependencies {
    // UniFFI's generated Kotlin loads the engine through JNA; this is its only
    // runtime dependency.
    implementation("net.java.dev.jna:jna:5.14.0@aar")
    testImplementation("junit:junit:4.13.2")
}
