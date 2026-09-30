package com.yet.tor

import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.alloc
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.ptr
import kotlinx.coroutines.delay
import platform.posix.F_GETFD
import platform.posix.RUSAGE_SELF
import platform.posix.fcntl
import platform.posix.getdtablesize
import platform.posix.getpid
import platform.posix.getrusage
import platform.posix.rusage
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Call inside the existing live test after bootstrap/HTTP are proved,
 * while the root is ready and client.sessions is empty. No additional bootstrap.
 * Logs numeric process/session counters only. Darwin ru_maxrss is BYTES and
 * a lifetime peak, not instantaneous RSS. FD scan is a best-effort snapshot;
 * Tor background networking can open/close descriptors during a sample.
 */
@OptIn(ExperimentalForeignApi::class)
private fun printIosResources(stage: String, sessions: Int) = memScoped {
    val limit = getdtablesize()
    check(limit > 0) { "getdtablesize failed" }
    var openDescriptors = 0
    for (fd in 0 until limit) {
        if (fcntl(fd, F_GETFD) >= 0) openDescriptors++
    }
    val usage = alloc<rusage>()
    check(getrusage(RUSAGE_SELF, usage.ptr) == 0) { "getrusage failed" }
    println("IOS_RESOURCE,stage=$stage,pid=${getpid()},sessions=$sessions,fd=$openDescriptors,peak_rss_bytes=${usage.ru_maxrss}")
}

/** Add this helper to TorIosE2ETest.kt, or keep as an internal test-only helper. */
internal suspend fun measureLiveSessionResources(client: ArtiTorClient) {
    assertTrue(client.isReady)
    assertTrue(client.sessions.isEmpty(), "resource baseline requires zero additional sessions")
    val rootEndpoint = requireNotNull(client.socksEndpoint)
    val owned = mutableListOf<TorIsolationSession>()
    delay(3_000)
    printIosResources("root-baseline", 0)
    try {
        for (target in listOf(1, 8, 16, 32)) {
            while (owned.size < target) {
                owned += client.createIsolationSession().getOrThrow()
            }
            assertEquals(target, client.sessions.size)
            assertEquals(target, owned.map { it.id }.toSet().size)
            assertTrue(owned.all { it.status.value.state == TorIsolationSessionState.ACTIVE })
            val ports = owned.map { requireNotNull(it.status.value.socksEndpoint).port }
            assertEquals(target + 1, (ports + rootEndpoint.port).toSet().size)
            delay(3_000)
            repeat(3) { sample ->
                printIosResources("active-$target-sample-$sample", target)
                delay(500)
            }
        }
        // Verify the frozen cap without retaining an unexpected extra session.
        val excess = client.createIsolationSession()
        excess.getOrNull()?.close()
        assertTrue(excess.isFailure, "33rd live session must be refused")
        assertEquals(32, client.sessions.size)
    } finally {
        owned.forEach { it.close() }
        assertTrue(client.sessions.isEmpty())
        delay(3_000)
        printIosResources("closed-all-root-retained", 0)
    }
}

