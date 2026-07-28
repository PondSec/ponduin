plugins {
    kotlin("jvm") version "2.2.21"
    `java-library`
    id("com.vanniktech.maven.publish") version "0.34.0"
}

group = "io.github.PondSec"
version = ponduinSdkVersion()

fun ponduinSdkVersion(): String {
    val cargoToml = file("../Cargo.toml").readText()
    return Regex("(?m)^version\\s*=\\s*\"([^\"]+)\"")
        .find(cargoToml)
        ?.groupValues
        ?.get(1)
        ?: error("Could not find ponduin-sdk version in ../Cargo.toml")
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_11
    targetCompatibility = JavaVersion.VERSION_11
    withSourcesJar()
}

dependencies {
    api("net.java.dev.jna:jna:5.14.0")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
}

tasks.jar {
    manifest {
        attributes(
            "Implementation-Title" to "Ponduin SDK",
            "Implementation-Version" to project.version,
        )
    }
}

mavenPublishing {
    publishToMavenCentral(automaticRelease = true)
    if (providers.gradleProperty("signingInMemoryKey").isPresent) {
        signAllPublications()
    }

    coordinates(
        groupId = "io.github.PondSec",
        artifactId = "pdk",
        version = project.version.toString(),
    )

    pom {
        name.set("Ponduin PDK")
        description.set("Kotlin/JVM bindings for the Ponduin SDK")
        inceptionYear.set("2026")
        url.set("https://github.com/PondSec/ponduin")
        licenses {
            license {
                name.set("PondSec Ponduin Software License Agreement")
                url.set("https://github.com/PondSec/ponduin/blob/dev/LICENSE")
                distribution.set("repo")
            }
        }
        developers {
            developer {
                id.set("pondsec")
                name.set("PondSec")
            }
        }
        scm {
            connection.set("scm:git:https://github.com/PondSec/ponduin.git")
            developerConnection.set("scm:git:ssh://git@github.com/PondSec/ponduin.git")
            url.set("https://github.com/PondSec/ponduin")
        }
    }
}
