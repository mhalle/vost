plugins {
    kotlin("jvm") version "2.1.0"
    id("com.gradleup.shadow") version "8.3.0"
}

group = "io.github.mhalle"
version = "0.9.9"

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(21)
    }
}

repositories {
    mavenCentral()
}

dependencies {
    implementation("org.eclipse.jgit:org.eclipse.jgit:7.1.0.202411261347-r")
    implementation("com.google.code.gson:gson:2.11.0")
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.0")
}

tasks.test {
    useJUnitPlatform()
}

tasks.shadowJar {
    archiveBaseName.set("vost-interop")
    archiveClassifier.set("")
    archiveVersion.set("")
    manifest {
        attributes["Main-Class"] = "vost.interop.MainKt"
    }
}
