plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

fun prop(name: String, default: String): String =
    (project.findProperty(name) as String?)?.takeIf { it.isNotBlank() } ?: default

fun propOrNull(name: String): String? =
    (project.findProperty(name) as String?)?.takeIf { it.isNotBlank() }

val nirAppId = prop("nirAppId", "org.nir.player")
val nirAppLabel = prop("nirAppLabel", "NIR Player")
val nirVersionCode = prop("nirVersionCode", "1").toInt()
val nirVersionName = prop("nirVersionName", "0.2.0")
val nirMinSdk = prop("nirMinSdk", "26").toInt()
val nirTargetSdk = prop("nirTargetSdk", "35").toInt()
val nirOrientation = prop("nirOrientation", "unspecified")
val nirPakPath = propOrNull("nirPakPath")
val nirIconPath = propOrNull("nirIconPath")
val nirJniLibsDir = propOrNull("nirJniLibsDir")

val stagedAssetsDir = layout.buildDirectory.dir("generated/nirAssets")
val stagedResDir = layout.buildDirectory.dir("generated/nirRes")
val stagedJniLibsDir = layout.buildDirectory.dir("generated/nirJniLibs")

val stageNirPak by tasks.registering {
    val pakFile = nirPakPath?.let { file(it) }?.takeIf { it.isFile }
    inputs.property("nirPakPath", pakFile?.absolutePath ?: "")
    if (pakFile != null) {
        inputs.file(pakFile)
    }
    outputs.dir(stagedAssetsDir)
    doLast {
        val outDir = stagedAssetsDir.get().asFile
        outDir.deleteRecursively()
        outDir.mkdirs()
        if (pakFile != null) {
            pakFile.copyTo(outDir.resolve("game.nirpak"), overwrite = true)
        }
    }
}

val stageNirIcon by tasks.registering {
    val customIconFile = nirIconPath?.let { file(it) }?.takeIf { it.isFile }
    inputs.property("nirIconPath", customIconFile?.absolutePath ?: "")
    if (customIconFile != null) {
        inputs.file(customIconFile)
    }
    outputs.dir(stagedResDir)
    doLast {
        val outDir = stagedResDir.get().asFile
        outDir.deleteRecursively()
        outDir.mkdirs()
        if (customIconFile != null) {
            val nodpi = outDir.resolve("drawable-nodpi").apply { mkdirs() }
            customIconFile.copyTo(nodpi.resolve("ic_launcher_custom.png"), overwrite = true)
            val xxxhdpi = outDir.resolve("mipmap-xxxhdpi").apply { mkdirs() }
            customIconFile.copyTo(xxxhdpi.resolve("ic_launcher.png"), overwrite = true)
        }
        val anydpiDir = outDir.resolve("mipmap-anydpi-v26").apply { mkdirs() }
        val fg = if (customIconFile != null) "@drawable/ic_launcher_custom" else "@drawable/ic_launcher_foreground"
        anydpiDir.resolve("ic_launcher.xml").writeText(
            """
            <?xml version="1.0" encoding="utf-8"?>
            <adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
                <background android:drawable="@drawable/ic_launcher_background" />
                <foreground android:drawable="$fg" />
            </adaptive-icon>
            """.trimIndent()
        )
    }
}

val stageNirJniLibs by tasks.registering(Sync::class) {
    into(stagedJniLibsDir)
    val explicitDir = nirJniLibsDir?.let { file(it) }
    val defaultSdkLibDir = rootProject.projectDir.resolve("../../../dist/sdk/android/lib").canonicalFile
    val sourceDir = when {
        explicitDir != null && explicitDir.isDirectory -> explicitDir
        defaultSdkLibDir.isDirectory -> defaultSdkLibDir
        else -> null
    }
    if (sourceDir != null) {
        from(sourceDir) {
            include("*/libplayer.so")
        }
    }
}

android {
    namespace = "org.nir.player"
    compileSdk = 35
    buildToolsVersion = "35.0.0"

    defaultConfig {
        applicationId = nirAppId
        minSdk = nirMinSdk
        targetSdk = nirTargetSdk
        versionCode = nirVersionCode
        versionName = nirVersionName

        manifestPlaceholders["nirAppLabel"] = nirAppLabel
        manifestPlaceholders["nirScreenOrientation"] = nirOrientation

        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    val releaseStoreFile = propOrNull("nirStoreFile")
    signingConfigs {
        if (releaseStoreFile != null) {
            create("nirRelease") {
                storeFile = file(releaseStoreFile)
                storePassword = prop("nirStorePassword", "android")
                keyAlias = prop("nirKeyAlias", "nir")
                keyPassword = prop("nirKeyPassword", "android")
                enableV1Signing = false
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            isShrinkResources = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            signingConfig = if (releaseStoreFile != null) {
                signingConfigs.getByName("nirRelease")
            } else {
                signingConfigs.getByName("debug")
            }
        }
        debug {
            isMinifyEnabled = false
        }
    }

    sourceSets {
        getByName("main") {
            assets.srcDir(stagedAssetsDir)
            jniLibs.srcDir(stagedJniLibsDir)
            res.srcDir(stagedResDir)
        }
    }

    // Keep `.nirpak` uncompressed for zero-copy `mmap()` directly from the APK!
    androidResources {
        noCompress += listOf("nirpak")
    }

    // AGP 8.7+ aligns uncompressed `.so` files to 16 KiB page boundaries for Android 15+ compliance.
    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

tasks.named("preBuild").configure {
    dependsOn(stageNirPak, stageNirIcon, stageNirJniLibs)
}

dependencies {
    // Must match the AGDK GameActivity 4.4.0 C++ glue bundled in `android-activity 0.6.1`.
    implementation("androidx.games:games-activity:4.4.0")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.appcompat:appcompat:1.7.0")
}
