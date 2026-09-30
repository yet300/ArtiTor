package com.yet.tor

import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import platform.Foundation.NSTemporaryDirectory
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/** Real UniFFI/facade validation with no bootstrap or network connections. */
class BridgeConfigurationTest {
    @Test
    fun malformedBridgeMatrixRemainsPublicConfigAndSecretSafe() = runBlocking<Unit> {
        val direct = "203.0.113.44:49123 SECRET_BRIDGE_FINGERPRINT_MARKER"
        val pt = "obfs4 203.0.113.44:49123 $1111111111111111111111111111111111111111 password=SECRET_TRANSPORT_OPTION_MARKER"
        for (mode in BridgesEnabled.entries) {
            for (line in listOf(direct, pt, "", " ", "\t", "\n")) {
                val client = ArtiTorClient()
                val logs = mutableListOf<String>()
                val collector = launch(start = CoroutineStart.UNDISPATCHED) {
                    client.logs.collect { logs += it }
                }
                try {
                    val result = client.start(
                        ArtiConfig(
                            dataDir = NSTemporaryDirectory() + "arti-bridge-validation",
                            bridges = listOf(line),
                            bridgesEnabled = mode,
                        ),
                        timeout = 1.seconds,
                    )
                    val error = assertIs<ArtiException.Config>(result.exceptionOrNull())
                    assertFalse(client.hasClient)
                    assertFalse(client.isReady)
                    val diagnostics = error.toString() + logs.joinToString()
                    for (secret in listOf("203.0.113.44", "49123", "SECRET_BRIDGE_FINGERPRINT_MARKER", "SECRET_TRANSPORT_OPTION_MARKER", "$1111111111111111111111111111111111111111")) {
                        assertFalse(diagnostics.contains(secret), "bridge material escaped")
                    }
                } finally {
                    client.shutdown()
                    collector.cancelAndJoin()
                }
            }
        }
    }

    @Test
    fun bridgesRequiredWithoutLinesFailsBeforeBootstrap() = runBlocking<Unit> {
        val client = ArtiTorClient()
        try {
            val result = client.start(
                ArtiConfig(dataDir = NSTemporaryDirectory() + "arti-bridge-required", bridgesEnabled = BridgesEnabled.ON),
                timeout = 1.seconds,
            )
            assertIs<ArtiException.Config>(result.exceptionOrNull())
            assertFalse(client.hasClient)
            assertTrue(client.sessions.isEmpty())
            assertFalse(client.isReady)
        } finally {
            client.shutdown()
        }
    }
}
