package com.yet.tor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class LifecycleInvariantTest {

    @Test
    fun offInvariants() {
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(TorState.OFF, false, null, 0, null),
        )
        assertTrue(
            checkLifecycleInvariants(TorState.OFF, true, null, 0, null).isNotEmpty(),
            "OFF with client must violate",
        )
        assertTrue(
            checkLifecycleInvariants(TorState.OFF, false, 9050, 0, null).isNotEmpty(),
            "OFF with SOCKS port must violate",
        )
    }

    @Test
    fun startingBootstrappingReportNoReadySocks() {
        for (s in listOf(TorState.STARTING, TorState.BOOTSTRAPPING)) {
            assertEquals(emptyList(), checkLifecycleInvariants(s, false, null, 12, null))
            assertTrue(
                checkLifecycleInvariants(s, false, 9050, 12, null).isNotEmpty(),
                "$s with SOCKS port must violate",
            )
        }
    }

    @Test
    fun runningInvariants() {
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(TorState.RUNNING, true, 9050, 100, null),
        )
        assertTrue(checkLifecycleInvariants(TorState.RUNNING, false, 9050, 100, null).isNotEmpty())
        assertTrue(checkLifecycleInvariants(TorState.RUNNING, true, null, 100, null).isNotEmpty())
        assertTrue(checkLifecycleInvariants(TorState.RUNNING, true, 9050, 42, null).isNotEmpty())
    }

    @Test
    fun pausedInvariants() {
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(TorState.PAUSED, true, null, 100, null),
        )
        assertTrue(checkLifecycleInvariants(TorState.PAUSED, false, null, 100, null).isNotEmpty())
        assertTrue(checkLifecycleInvariants(TorState.PAUSED, true, 9050, 100, null).isNotEmpty())
        assertTrue(checkLifecycleInvariants(TorState.PAUSED, true, null, 42, null).isNotEmpty())
    }

    @Test
    fun errorRequiresTypedErrorAndNoReadiness() {
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(
                TorState.ERROR, false, null, 0, ArtiException.Bootstrap("x"),
            ),
        )
        assertTrue(
            checkLifecycleInvariants(TorState.ERROR, false, null, 0, null).isNotEmpty(),
            "ERROR without typed error must violate",
        )
        assertTrue(
            checkLifecycleInvariants(
                TorState.ERROR, false, 9050, 0, ArtiException.Runtime("x"),
            ).isNotEmpty(),
            "ERROR with SOCKS port must violate",
        )
    }

    @Test
    fun readyDefinitionMatchesSpec() {
        val ready = TorStatus(TorState.RUNNING, 100, 9050, "", null)
        assertTrue(ready.isReady)
        assertTrue(!TorStatus(TorState.RUNNING, 99, 9050, "", null).isReady)
        assertTrue(!TorStatus(TorState.RUNNING, 100, null, "", null).isReady)
        assertTrue(!TorStatus(TorState.PAUSED, 100, null, "", null).isReady)
    }
}
