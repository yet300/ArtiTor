package consumer
import com.yet.tor.*
import kotlin.time.Duration.Companion.milliseconds

fun publishedConfig() = ArtiConfig(
    dataDir = "/tmp/consumer", bridgesEnabled = BridgesEnabled.OFF,
    allowOnionAddrs = false, connectTimeout = 1200.milliseconds,
    resolveTimeout = 2400.milliseconds,
)
fun publishedKind(failure: ArtiException): TorErrorKind = failure.kind
fun publishedSessionState(session: TorIsolationSession): TorIsolationSessionState = session.status.value.state
