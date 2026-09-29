package com.yet.tor

import com.yet.tor.ffi.ArtiConfig as FfiConfig
import com.yet.tor.ffi.ArtiErrorDetail as FfiErrorDetail
import com.yet.tor.ffi.ArtiException as FfiArtiException
import com.yet.tor.ffi.ArtiTorInterface as FfiArtiTorInterface
import com.yet.tor.ffi.ErrorKind as FfiErrorKind
import com.yet.tor.ffi.StatusListener
import com.yet.tor.ffi.TorState as FfiTorState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlin.time.TimeSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking

/**
 * Deterministic facade tests against a scripted fake native layer (no Tor
 * network, no native library). Live bootstrap/pause/resume over the real
 * network remains covered by the Android/iOS E2E tests.
 */
class ArtiTorClientLifecycleTest {

    private class FakeNative(val scope: CoroutineScope) : FfiArtiTorInterface {
        var hasClientValue = false
        var readyPort: Int? = null
        val startCalls = mutableListOf<FfiConfig>()
        var pauseCalls = 0
        var shutdownCalls = 0
        var resumeCalls = 0
        var startBehavior: (FfiConfig, StatusListener) -> Unit = { _, _ -> }
        var resumeBehavior: (StatusListener) -> Unit = { _ -> }
        var pauseBehavior: () -> Unit = {}
        var shutdownBehavior: () -> Unit = {}

        override fun version(): String = "fake"
        override fun hasClient(): Boolean = hasClientValue
        override fun isReady(): Boolean = readyPort != null && hasClientValue
        override fun socksPort(): UShort? = readyPort?.toUShort()
        override fun start(config: FfiConfig, listener: StatusListener) {
            startCalls += config
            startBehavior(config, listener)
        }
        override fun resume(listener: StatusListener) {
            resumeCalls++
            resumeBehavior(listener)
        }
        override fun pause() {
            pauseCalls++
            pauseBehavior()
        }
        override fun shutdown() {
            shutdownCalls++
            shutdownBehavior()
        }
        override fun stop() = shutdown()
    }

    private fun readyFake(
        scope: CoroutineScope,
        port: Int = 19050,
        async: Boolean = false,
    ): FakeNative {
        val fake = FakeNative(scope)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            listener.onStatus(FfiTorState.STARTING, 0u, null, "starting")
            val emit = {
                fake.hasClientValue = true
                fake.readyPort = port
                listener.onStatus(FfiTorState.RUNNING, 100u, port.toUShort(), "proxy ready")
            }
            if (async) scope.launch { delay(50); emit() } else emit()
        }
        fake.pauseBehavior = {
            fake.readyPort = null
            // Native reports synchronously; bootstrap stays 100, client kept.
            lastListener?.onStatus(FfiTorState.PAUSED, 100u, null, "paused")
        }
        fake.resumeBehavior = { listener ->
            if (!fake.hasClientValue) throw FfiArtiException.NotRunning()
            fake.readyPort = port
            listener.onStatus(FfiTorState.RUNNING, 100u, port.toUShort(), "proxy ready")
        }
        fake.shutdownBehavior = {
            fake.hasClientValue = false
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.OFF, 0u, null, "")
        }
        return fake
    }

    companion object {
        // Tracks the most recent listener for pause/shutdown emulation.
        var lastListener: StatusListener? = null
    }

    @Test
    fun coldStartReachesReadyAndSavesConfig() = runBlocking<Unit> {
        lastListener = null
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        val config = ArtiConfig(dataDir = "/tmp/a", socksPort = 0)

        val result = client.start(config, timeout = 5.seconds)
        assertTrue(result.isSuccess, "start failed: ${result.exceptionOrNull()}")
        val status = client.status.value
        assertEquals(TorState.RUNNING, status.state)
        assertEquals(100, status.bootstrapPercent)
        assertEquals(19050, status.socksPort)
        assertTrue(client.isReady)
        assertTrue(client.hasClient)
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(
                status.state, client.hasClient, status.socksPort,
                status.bootstrapPercent, status.lastError,
            ),
        )
        assertEquals(0, fake.startCalls.first().socksPort.toInt(), "ephemeral port passes through")
    }

    @Test
    fun fixedPortCollisionSurfacesTypedBind() = runBlocking<Unit> {
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            listener.onStatus(FfiTorState.STARTING, 0u, null, "starting")
            launch {
                delay(20)
                listener.onError(FfiErrorDetail(FfiErrorKind.BIND, 9060u, "address in use"))
                listener.onStatus(FfiTorState.ERROR, 0u, null, "error: bind")
            }
        }
        val client = ArtiTorClient(fake)
        val result = client.start(ArtiConfig(dataDir = "/tmp/a", socksPort = 9060), 5.seconds)
        assertTrue(result.isFailure)
        val err = assertIs<ArtiException.Bind>(result.exceptionOrNull())
        assertEquals(9060, err.port)
        assertEquals(TorState.ERROR, client.status.value.state)
        assertIs<ArtiException.Bind>(client.status.value.lastError)
        assertFalse(client.isReady)
    }

    @Test
    fun asyncBootstrapFailureSurfacesTypedBootstrap() = runBlocking<Unit> {
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            listener.onStatus(FfiTorState.BOOTSTRAPPING, 12u, null, "bootstrapping 12%")
            launch {
                delay(20)
                listener.onError(FfiErrorDetail(FfiErrorKind.BOOTSTRAP, null, "no consensus"))
                listener.onStatus(FfiTorState.ERROR, 0u, null, "error: bootstrap")
            }
        }
        val client = ArtiTorClient(fake)
        val result = client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds)
        assertIs<ArtiException.Bootstrap>(result.exceptionOrNull())
        assertIs<ArtiException.Bootstrap>(client.status.value.lastError)
    }

    @Test
    fun syncConfigFailureStaysConfig() = runBlocking<Unit> {
        val fake = FakeNative(this)
        fake.startBehavior = { _, _ -> throw FfiArtiException.Config("bad bridge line") }
        val client = ArtiTorClient(fake)
        val result = client.start(
            ArtiConfig(dataDir = "/tmp/a", bridges = listOf("junk")),
            5.seconds,
        )
        assertIs<ArtiException.Config>(result.exceptionOrNull())
    }

    @Test
    fun pauseAfterReadyKeepsClientAndResumeRebindsWithoutRestart() = runBlocking<Unit> {
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            fake.hasClientValue = true
            fake.readyPort = 19050
            listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "proxy ready")
        }
        fake.pauseBehavior = {
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.PAUSED, 100u, null, "paused")
        }
        fake.resumeBehavior = { listener ->
            if (!fake.hasClientValue) throw FfiArtiException.NotRunning()
            fake.readyPort = 19050
            listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "proxy ready")
        }
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).getOrThrow()

        client.pause()
        delay(50)
        assertFalse(client.isReady)
        assertTrue(client.hasClient)
        assertEquals(TorState.PAUSED, client.status.value.state)
        val paused = client.status.value
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(
                paused.state, true, paused.socksPort,
                paused.bootstrapPercent, paused.lastError,
            ),
        )

        val startsBeforeResume = fake.startCalls.size
        client.resume(timeout = 5.seconds).getOrThrow()
        assertTrue(client.isReady)
        assertEquals(
            startsBeforeResume,
            fake.startCalls.size,
            "resume must rebind without a new bootstrap/start",
        )
        assertEquals(1, fake.resumeCalls)
    }

    @Test
    fun shutdownClearsToOffAndRestartUsesSavedConfig() = runBlocking<Unit> {
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        val config = ArtiConfig(dataDir = "/tmp/saved")
        client.start(config, 5.seconds).getOrThrow()

        client.shutdown()
        assertEquals(TorState.OFF, client.status.value.state)
        assertFalse(client.hasClient)
        assertFalse(client.isReady)

        val result = client.restart(timeout = 5.seconds)
        assertTrue(result.isSuccess, "restart failed: ${result.exceptionOrNull()}")
        assertTrue(client.isReady)
        assertEquals("/tmp/saved", fake.startCalls.last().dataDir)
    }

    @Test
    fun restartUsesSuppliedConfigOverSaved() = runBlocking<Unit> {
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/first"), 5.seconds).getOrThrow()

        client.restart(ArtiConfig(dataDir = "/tmp/second"), 5.seconds).getOrThrow()
        assertEquals("/tmp/second", fake.startCalls.last().dataDir)
        assertTrue(client.isReady)
    }

    @Test
    fun restartWithoutAnyConfigFailsFast() = runBlocking<Unit> {
        val fake = FakeNative(this)
        val client = ArtiTorClient(fake)
        val result = client.restart(timeout = 5.seconds)
        assertIs<ArtiException.Config>(result.exceptionOrNull())
        assertEquals(0, fake.startCalls.size, "no native call without a config")
    }

    @Test
    fun concurrentDoubleStartJoinsInflightWork() = runBlocking<Unit> {
        val fake = FakeNative(this)
        var calls = 0
        fake.startBehavior = { _, listener ->
            lastListener = listener
            calls++
            if (calls == 1) {
                listener.onStatus(FfiTorState.STARTING, 0u, null, "starting")
                launch {
                    delay(100)
                    fake.hasClientValue = true
                    fake.readyPort = 19050
                    listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
                }
            } else {
                throw FfiArtiException.AlreadyRunning()
            }
        }
        val client = ArtiTorClient(fake)
        val a = async { client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds) }
        delay(20)
        val b = async { client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds) }
        assertTrue(a.await().isSuccess)
        assertTrue(b.await().isSuccess)
        assertTrue(client.isReady)
    }

    @Test
    fun shutdownDuringBootstrapCancelsFast() = runBlocking<Unit> {
        val gate = CompletableDeferred<Unit>()
        var defunct = false
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            listener.onStatus(FfiTorState.STARTING, 0u, null, "starting")
            launch {
                gate.await()
                if (!defunct) {
                    fake.hasClientValue = true
                    fake.readyPort = 19050
                    listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
                }
            }
        }
        fake.shutdownBehavior = {
            defunct = true
            fake.hasClientValue = false
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.OFF, 0u, null, "")
        }
        val client = ArtiTorClient(fake)

        val mark = TimeSource.Monotonic.markNow()
        val job = async { client.start(ArtiConfig(dataDir = "/tmp/a"), timeout = 30.seconds) }
        delay(150)
        assertEquals(TorState.STARTING, client.status.value.state)
        client.shutdown()
        val result = job.await()
        val elapsed = mark.elapsedNow()
        assertTrue(result.isFailure, "cancelled start must fail")
        assertIs<ArtiException.Runtime>(
            result.exceptionOrNull(),
            "cancelled start must fail fast with Runtime, not Timeout",
        )
        assertTrue(elapsed < 10.seconds, "must not wait out the timeout (took $elapsed)")
        assertEquals(TorState.OFF, client.status.value.state)
        gate.complete(Unit)
    }

    @Test
    fun pauseDuringBootstrapSettlesToOffAndCancelsWaiter() = runBlocking<Unit> {
        val gate = CompletableDeferred<Unit>()
        var defunct = false
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            listener.onStatus(FfiTorState.BOOTSTRAPPING, 30u, null, "bootstrapping 30%")
            launch {
                gate.await()
                if (!defunct) {
                    fake.hasClientValue = true
                    fake.readyPort = 19050
                    listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
                }
            }
        }
        fake.pauseBehavior = {
            // Native pause-during-bootstrap semantics: discard, report OFF.
            defunct = true
            fake.hasClientValue = false
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.OFF, 0u, null, "bootstrap cancelled")
        }
        val client = ArtiTorClient(fake)

        val mark = TimeSource.Monotonic.markNow()
        val job = async { client.start(ArtiConfig(dataDir = "/tmp/a"), timeout = 30.seconds) }
        delay(150)
        client.pause()
        val result = job.await()
        val elapsed = mark.elapsedNow()
        assertTrue(result.isFailure)
        assertIs<ArtiException.Runtime>(result.exceptionOrNull())
        assertTrue(elapsed < 10.seconds, "must not strand the waiter (took $elapsed)")
        assertEquals(TorState.OFF, client.status.value.state)
        assertFalse(client.hasClient)
        gate.complete(Unit)
    }

    @Test
    fun resumeThenShutdownCancelsWaiter() = runBlocking<Unit> {
        val gate = CompletableDeferred<Unit>()
        var defunct = false
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).getOrThrow()

        // Pause, then gate the resume rebind.
        lastListener?.onStatus(FfiTorState.PAUSED, 100u, null, "paused")
        fake.hasClientValue = true
        fake.readyPort = null
        fake.resumeBehavior = { listener ->
            launch {
                gate.await()
                if (!defunct) {
                    fake.readyPort = 19050
                    listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
                }
            }
        }
        fake.shutdownBehavior = {
            defunct = true
            fake.hasClientValue = false
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.OFF, 0u, null, "")
        }

        val mark = TimeSource.Monotonic.markNow()
        val job = async { client.resume(timeout = 30.seconds) }
        delay(150)
        client.shutdown()
        val result = job.await()
        assertTrue(result.isFailure)
        assertTrue(mark.elapsedNow() < 10.seconds)
        assertEquals(TorState.OFF, client.status.value.state)
        gate.complete(Unit)
    }

    @Test
    fun startAfterErrorRecovers() = runBlocking<Unit> {
        val fake = FakeNative(this)
        var calls = 0
        fake.startBehavior = { _, listener ->
            lastListener = listener
            calls++
            if (calls == 1) {
                launch {
                    delay(20)
                    listener.onError(FfiErrorDetail(FfiErrorKind.BOOTSTRAP, null, "no consensus"))
                    listener.onStatus(FfiTorState.ERROR, 0u, null, "error")
                }
            } else {
                fake.hasClientValue = true
                fake.readyPort = 19050
                listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
            }
        }
        val client = ArtiTorClient(fake)
        assertIs<ArtiException.Bootstrap>(
            client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).exceptionOrNull(),
        )
        assertEquals(TorState.ERROR, client.status.value.state)
        assertTrue(client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).isSuccess)
        assertTrue(client.isReady)
    }

    @Test
    fun startWhileRunningSameConfigIsNoop() = runBlocking<Unit> {
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        val config = ArtiConfig(dataDir = "/tmp/a", socksPort = 0)
        client.start(config, 5.seconds).getOrThrow()
        assertEquals(1, fake.startCalls.size)

        assertTrue(client.start(config, 5.seconds).isSuccess)
        assertEquals(1, fake.startCalls.size, "same effective config must not re-kick")
        assertEquals(0, fake.pauseCalls)
        assertEquals(0, fake.shutdownCalls)
    }

    @Test
    fun startWhileRunningPortOnlyChangeRebindsWithoutTeardown() = runBlocking<Unit> {
        val fake = readyFake(this, port = 19050)
        fake.pauseBehavior = {
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.PAUSED, 100u, null, "paused")
        }
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a", socksPort = 0), 5.seconds).getOrThrow()

        // Port-only change: pause + rebind, no full shutdown.
        fake.startBehavior = { config, listener ->
            lastListener = listener
            fake.readyPort = config.socksPort.toInt()
            listener.onStatus(FfiTorState.RUNNING, 100u, config.socksPort, "proxy ready")
        }
        assertTrue(
            client.start(ArtiConfig(dataDir = "/tmp/a", socksPort = 19060), 5.seconds).isSuccess,
        )
        assertEquals(1, fake.pauseCalls)
        assertEquals(0, fake.shutdownCalls)
        assertEquals(19060, fake.startCalls.last().socksPort.toInt())
        assertTrue(client.isReady)
    }

    @Test
    fun startWhileRunningClientDefiningChangeColdStarts() = runBlocking<Unit> {
        val fake = readyFake(this)
        fake.shutdownBehavior = {
            fake.hasClientValue = false
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.OFF, 0u, null, "")
        }
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).getOrThrow()

        assertTrue(
            client.start(
                ArtiConfig(dataDir = "/tmp/a", bridges = listOf("obfs4 1.2.3.4:443 FP")),
                5.seconds,
            ).isSuccess,
        )
        assertEquals(1, fake.shutdownCalls, "client-defining change must tear down first")
        assertEquals(
            listOf("obfs4 1.2.3.4:443 FP"),
            fake.startCalls.last().bridges,
        )
    }

    @Test
    fun startWhilePausedPassesNewPortThrough() = runBlocking<Unit> {
        val fake = readyFake(this, port = 19050)
        fake.pauseBehavior = {
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.PAUSED, 100u, null, "paused")
        }
        // Rebind path honors the supplied port.
        fake.startBehavior = { config, listener ->
            lastListener = listener
            launch {
                delay(20)
                fake.readyPort = config.socksPort.toInt()
                listener.onStatus(FfiTorState.RUNNING, 100u, config.socksPort, "ready")
            }
        }
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a", socksPort = 19050), 5.seconds).getOrThrow()
        client.pause()
        delay(50)
        assertEquals(TorState.PAUSED, client.status.value.state)

        assertTrue(
            client.start(ArtiConfig(dataDir = "/tmp/a", socksPort = 19060), 5.seconds).isSuccess,
        )
        assertEquals(19060, fake.startCalls.last().socksPort.toInt())
        assertTrue(client.isReady)
    }

    @Test
    fun startWhilePausedWithClearedBridgesPassesThemThrough() = runBlocking<Unit> {
        val fake = readyFake(this)
        fake.pauseBehavior = {
            fake.readyPort = null
            lastListener?.onStatus(FfiTorState.PAUSED, 100u, null, "paused")
        }
        fake.startBehavior = { config, listener ->
            lastListener = listener
            launch {
                delay(20)
                listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
            }
        }
        val client = ArtiTorClient(fake)
        client.start(
            ArtiConfig(dataDir = "/tmp/a", bridges = listOf("obfs4 1.2.3.4:443 FP")),
            5.seconds,
        ).getOrThrow()
        client.pause()
        delay(50)

        // Clearing bridges must reach the native layer (which rebuilds the
        // client) rather than being silently dropped.
        assertTrue(
            client.start(ArtiConfig(dataDir = "/tmp/a", bridges = emptyList()), 5.seconds).isSuccess,
        )
        assertTrue(fake.startCalls.last().bridges.isEmpty())
    }

    @Test
    fun startTimeoutSurfacesTimeout() = runBlocking<Unit> {
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            listener.onStatus(FfiTorState.BOOTSTRAPPING, 10u, null, "stuck")
            // Never completes.
        }
        val client = ArtiTorClient(fake)
        val result = client.start(ArtiConfig(dataDir = "/tmp/a"), timeout = 300.milliseconds)
        assertIs<ArtiException.Timeout>(result.exceptionOrNull())
    }

    @Test
    fun startAfterFacadeTimeoutJoinsInflightWorker() = runBlocking<Unit> {
        // The facade timed out but the native worker is still bootstrapping:
        // a second start must join it (AlreadyRunning) instead of kicking a
        // second bootstrap.
        val gate = CompletableDeferred<Unit>()
        val fake = FakeNative(this)
        var kicks = 0
        fake.startBehavior = { _, listener ->
            lastListener = listener
            kicks++
            if (kicks == 1) {
                listener.onStatus(FfiTorState.STARTING, 0u, null, "starting")
                launch {
                    gate.await()
                    fake.hasClientValue = true
                    fake.readyPort = 19050
                    listener.onStatus(FfiTorState.RUNNING, 100u, 19050u, "ready")
                }
            } else {
                throw FfiArtiException.AlreadyRunning()
            }
        }
        val client = ArtiTorClient(fake)
        val config = ArtiConfig(dataDir = "/tmp/a")

        assertIs<ArtiException.Timeout>(
            client.start(config, timeout = 300.milliseconds).exceptionOrNull(),
        )
        assertEquals(1, kicks)

        // The worker is still bootstrapping: the next start joins it instead
        // of kicking a second bootstrap.
        val second = async { client.start(config, timeout = 5.seconds) }
        delay(200)
        assertEquals(2, kicks, "second start must attempt the native kick")
        gate.complete(Unit)
        assertTrue(second.await().isSuccess, "second start must join in-flight work")
        assertTrue(client.isReady)
    }

    @Test
    fun pausePauseAndShutdownShutdownAreIdempotent() = runBlocking<Unit> {
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).getOrThrow()

        client.pause()
        client.pause()
        assertEquals(TorState.PAUSED, client.status.value.state)

        client.shutdown()
        client.shutdown()
        assertEquals(TorState.OFF, client.status.value.state)
        assertFalse(client.hasClient)
    }

    @Test
    fun shutdownThenStartAgainWorks() = runBlocking<Unit> {
        val fake = readyFake(this)
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).getOrThrow()
        client.shutdown()
        assertTrue(client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds).isSuccess)
        assertTrue(client.isReady)
    }

    @Test
    fun errorStatusCarriesTypedErrorAndNoReadiness() = runBlocking<Unit> {
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            launch {
                delay(20)
                listener.onError(FfiErrorDetail(FfiErrorKind.RUNTIME, null, "boom"))
                listener.onStatus(FfiTorState.ERROR, 50u, null, "error")
            }
        }
        val client = ArtiTorClient(fake)
        client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds)
        val status = client.status.value
        assertIs<ArtiException.Runtime>(status.lastError)
        assertEquals(
            emptyList(),
            checkLifecycleInvariants(
                status.state, client.hasClient, status.socksPort,
                status.bootstrapPercent, status.lastError,
            ),
        )
    }

    @Test
    fun lateOnErrorStillUpgradesToTyped() = runBlocking<Unit> {
        // Native guarantees onError precedes onStatus(Error); verify the
        // reverse arrival order also cannot downgrade the type to Runtime.
        val fake = FakeNative(this)
        fake.startBehavior = { _, listener ->
            lastListener = listener
            launch {
                delay(20)
                listener.onStatus(FfiTorState.ERROR, 0u, null, "error: bind")
                listener.onError(FfiErrorDetail(FfiErrorKind.BIND, 9060u, "address in use"))
            }
        }
        val client = ArtiTorClient(fake)
        val result = client.start(ArtiConfig(dataDir = "/tmp/a"), 5.seconds)
        assertIs<ArtiException.Bind>(result.exceptionOrNull())
    }
}
