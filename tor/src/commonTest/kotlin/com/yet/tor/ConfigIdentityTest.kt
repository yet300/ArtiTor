package com.yet.tor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ConfigIdentityTest {

    private fun base() = ArtiConfig(dataDir = "/tmp/artitor-test")

    @Test
    fun identicalConfigNeedsNoNewClient() {
        assertFalse(torClientConfigChanged(base(), base()))
    }

    @Test
    fun socksPortOnlyNeedsNoNewClient() {
        assertFalse(torClientConfigChanged(base(), base().copy(socksPort = 19050)))
    }

    @Test
    fun dataDirChangeNeedsNewClient() {
        assertTrue(torClientConfigChanged(base(), base().copy(dataDir = "/tmp/other")))
    }

    @Test
    fun stateDirChangeNeedsNewClient() {
        assertTrue(torClientConfigChanged(base(), base().copy(stateDir = "/tmp/s")))
        val a = base().copy(stateDir = "/tmp/s1")
        assertTrue(torClientConfigChanged(a, a.copy(stateDir = "/tmp/s2")))
    }

    @Test
    fun cacheDirChangeNeedsNewClient() {
        assertTrue(torClientConfigChanged(base(), base().copy(cacheDir = "/tmp/c")))
    }

    @Test
    fun bridgesEmptyToNonEmptyNeedsNewClient() {
        assertTrue(
            torClientConfigChanged(
                base(),
                base().copy(bridges = listOf("obfs4 1.2.3.4:443 FINGERPRINT")),
            ),
        )
    }

    @Test
    fun bridgesNonEmptyToEmptyNeedsNewClient() {
        // Clearing bridges must rebuild: the old client was built with bridges.
        val withBridges = base().copy(bridges = listOf("obfs4 1.2.3.4:443 FINGERPRINT"))
        assertTrue(torClientConfigChanged(withBridges, base()))
    }

    @Test
    fun sameBridgesNeedNoNewClient() {
        val a = base().copy(bridges = listOf("obfs4 1.2.3.4:443 FINGERPRINT"))
        assertFalse(torClientConfigChanged(a, a.copy()))
    }

    @Test
    fun bridgeDefaultsPreserveExistingCallers() {
        assertEquals(emptyList(), base().bridges)
        assertEquals(BridgesEnabled.AUTO, base().bridgesEnabled)
        assertEquals(BridgesEnabled.AUTO, ArtiConfig(dataDir = "/tmp/a", bridges = listOf("fixture")).bridgesEnabled)
        val oldPositional = ArtiConfig("/tmp/a", 9050, listOf("fixture"), "/tmp/s", "/tmp/c")
        assertEquals("/tmp/s", oldPositional.stateDir)
        assertEquals("/tmp/c", oldPositional.cacheDir)
        assertEquals(BridgesEnabled.AUTO, oldPositional.bridgesEnabled)
    }

    @Test
    fun bridgeIdentityUsesExactLinesAndEveryMode() {
        for (mode in BridgesEnabled.entries) {
            val a = base().copy(bridges = listOf("A", "B"), bridgesEnabled = mode)
            assertFalse(torClientConfigChanged(a, a.copy()))
            assertFalse(torClientConfigChanged(a, a.copy(socksPort = 19050)))
            for (other in BridgesEnabled.entries) {
                assertEquals(mode != other, torClientConfigChanged(a, a.copy(bridgesEnabled = other)))
            }
            for (lines in listOf(listOf("B", "A"), listOf(" A", "B"), listOf("B"), emptyList())) {
                assertTrue(torClientConfigChanged(a, a.copy(bridges = lines)))
            }
        }
    }
}
