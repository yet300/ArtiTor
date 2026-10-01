package consumer

import com.yet.tor.*

/** Compile-only consumer fixture: these calls use the complete 0.2 surface. */
@Suppress("unused", "DEPRECATION")
private suspend fun compile02Consumer(client: ArtiTorClient, failure: ArtiException) {
    val one = ArtiConfig("/tmp/compat")
    val two = ArtiConfig("/tmp/compat", 9050)
    val three = ArtiConfig("/tmp/compat", 9050, emptyList())
    val four = ArtiConfig("/tmp/compat", 9050, emptyList(), "/tmp/state")
    val positional = ArtiConfig("/tmp/compat", 9050, emptyList(), "/tmp/state", "/tmp/cache")
    val named = ArtiConfig(
        dataDir = "/tmp/compat", socksPort = 9050, bridges = emptyList(),
        stateDir = "/tmp/state", cacheDir = "/tmp/cache",
    )
    client.start(positional)
    client.pause()
    client.resume()
    client.restart(named)
    client.restart()
    val status: TorStatus = client.status.value
    val port: Int? = status.socksPort
    val ready: Boolean = status.isReady && client.isReady
    val held: Boolean = client.hasClient
    val version: String = client.version
    client.logs
    client.stop()
    client.shutdown()
    val exhaustive = when (failure) {
        is ArtiException.AlreadyRunning -> "already"
        is ArtiException.NotRunning -> "not-running"
        is ArtiException.Config -> "config"
        is ArtiException.Bind -> failure.port.toString()
        is ArtiException.Bootstrap -> "bootstrap"
        is ArtiException.Timeout -> "timeout"
        is ArtiException.Runtime -> "runtime"
    }
}
