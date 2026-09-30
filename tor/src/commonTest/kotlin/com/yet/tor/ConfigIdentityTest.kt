package com.yet.tor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

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
    @Test
    fun onionAndTimeoutDefaultsAndIdentity() {
        val good = base()
        assertTrue(good.allowOnionAddrs)
        assertEquals(10.seconds, good.connectTimeout)
        assertEquals(10.seconds, good.resolveTimeout)
        for (candidate in listOf(
            good.copy(allowOnionAddrs = false),
            good.copy(connectTimeout = 5.seconds),
            good.copy(resolveTimeout = 5.seconds),
        )) {
            assertTrue(torClientConfigChanged(good, candidate))
            assertTrue(torClientConfigChanged(candidate, good))
            assertFalse(torClientConfigChanged(candidate, candidate.copy(socksPort = 9050)))
        }
    }

    /** Same 512-case table/oracle as native identity_parity_512_cases. */
    @Test fun identityParity512Cases() {
        val original = base()
        for (mask in 0 until 512) {
            val changed = original.copy(
                dataDir = if (mask and 1 != 0) "/tmp/other" else original.dataDir,
                socksPort = if (mask and 2 != 0) 19050 else original.socksPort,
                bridges = if (mask and 4 != 0) listOf("B", "A") else original.bridges,
                stateDir = if (mask and 8 != 0) "/tmp/s" else original.stateDir,
                cacheDir = if (mask and 16 != 0) "/tmp/c" else original.cacheDir,
                bridgesEnabled = if (mask and 32 != 0) BridgesEnabled.ON else original.bridgesEnabled,
                allowOnionAddrs = if (mask and 64 != 0) false else original.allowOnionAddrs,
                connectTimeout = if (mask and 128 != 0) 5.seconds else original.connectTimeout,
                resolveTimeout = if (mask and 256 != 0) 5.seconds else original.resolveTimeout,
            )
            val expected = mask and 2.inv() != 0
            assertEquals(expected, torClientConfigChanged(original, changed), "mask=$mask forward")
            assertEquals(expected, torClientConfigChanged(changed, original), "mask=$mask reverse")
            assertFalse(torClientConfigChanged(changed, changed.copy()), "mask=$mask identical")
        }
    }

}
