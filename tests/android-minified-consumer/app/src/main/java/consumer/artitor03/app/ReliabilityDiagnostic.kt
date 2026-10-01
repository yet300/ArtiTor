package consumer.artitor03.app

import android.content.Context
import com.yet.tor.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import java.io.File
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.Socket
import java.util.Collections
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory
import kotlin.time.Duration.Companion.seconds

/** Finite hardware probes; local phases never send SOCKS CONNECT or target DNS. */
internal class ReliabilityDiagnostic(private val context: Context, private val emit: (String) -> Unit) {
    private val history = Collections.synchronizedList(mutableListOf<String>())
    private var roots = 0
    private var sessions = 0
    private var resumes = 0
    private var colds = 0

    private fun greeting(port: Int, label: String) {
        val at = System.nanoTime()
        var stage = "loopback_connect"
        try {
            Socket().use { socket ->
                socket.connect(InetSocketAddress("127.0.0.1", port), 5000)
                stage = "socks_greeting"
                socket.soTimeout = 5000
                socket.getOutputStream().apply { write(byteArrayOf(5, 1, 0)); flush() }
                val input = socket.getInputStream()
                check(input.read() == 5 && input.read() == 0) { "unexpected greeting reply" }
            }
            emit("LOCAL_PASS,$label,port=$port,at_ns=$at,end_ns=${System.nanoTime()}")
        } catch (t: Throwable) {
            emit("LOCAL_FAILURE,$label,port=$port,stage=$stage,at_ns=$at,type=${t.javaClass.name},message=${t.message}")
            emit("LOCAL_HISTORY,${synchronized(history) { history.toList().joinToString("|") }}")
            throw t // Stop on the first published endpoint failure; never retry it.
        }
    }

    private fun refused(port: Int, label: String, tor: ArtiTorClient, session: TorIsolationSession, pauseAt: Long) {
        val at = System.nanoTime()
        try {
            Socket().use { socket ->
                socket.connect(InetSocketAddress("127.0.0.1", port), 1500)
                emit("OLD_ACCEPTED,$label,port=$port,pause_return_ns=$pauseAt,connect_at_ns=$at,connected_ns=${System.nanoTime()},root=${tor.status.value},hasClient=${tor.hasClient},session=${session.status.value}")
                emit("LOCAL_HISTORY,${synchronized(history) { history.takeLast(24).joinToString("|") }}")
                // Diagnose the already-failed negative TCP probe without opening a
                // new connection or ever sending CONNECT. This cannot turn it green.
                val reply = runCatching {
                    socket.soTimeout = 1500
                    socket.getOutputStream().apply { write(byteArrayOf(5, 1, 0)); flush() }
                    val input = socket.getInputStream()
                    "${input.read()},${input.read()}"
                }.fold({ "reply=$it" }, { "type=${it.javaClass.name},message=${it.message}" })
                emit("OLD_ACCEPTED_GREETING,$label,$reply")
            }
            error("old listener still accepts: $label port=$port")
        } catch (t: ConnectException) {
            // Android exposes the errno cause. Timeouts and other IO errors do not pass.
            val errno = generateSequence(t as Throwable?) { it.cause }
                .filterIsInstance<android.system.ErrnoException>().firstOrNull()?.errno
            check(errno == android.system.OsConstants.ECONNREFUSED) { "not ECONNREFUSED: $t" }
            emit("OLD_REFUSED,$label,port=$port,errno=$errno")
        }
    }

    private suspend fun root(tor: ArtiTorClient): Int = requireNotNull(withTimeout(30_000) {
        tor.status.first { it.state == TorState.RUNNING && it.socksPort != null }
    }.socksPort)

    private suspend fun active(session: TorIsolationSession): Int = requireNotNull(withTimeout(30_000) {
        session.status.first { it.state == TorIsolationSessionState.ACTIVE && it.socksEndpoint != null }
    }.socksEndpoint).port

    private fun https(tor: ArtiTorClient, port: Int, host: String, path: String, label: String) {
        var stage = "loopback_connect"
        var code: Int? = null
        val snapshot = tor.status.value
        emit("EXTERNAL_BEGIN,$label,identity=root,port=$port,state=${snapshot.state},bootstrap=${snapshot.bootstrapPercent},hasClient=${tor.hasClient},target=$host")
        try {
            Socket().use { socket ->
                socket.connect(InetSocketAddress("127.0.0.1", port), 5000)
                socket.soTimeout = 90000
                val out = socket.getOutputStream(); val input = socket.getInputStream()
                stage = "socks_greeting"
                out.write(byteArrayOf(5, 1, 0)); out.flush()
                check(input.read() == 5 && input.read() == 0)
                stage = "socks_connect_reply"
                val name = host.toByteArray(Charsets.US_ASCII)
                out.write(byteArrayOf(5, 1, 0, 3, name.size.toByte()) + name + byteArrayOf(1, -69)); out.flush()
                check(input.read() == 5); code = input.read()
                emit("SOCKS_REPLY,$label,code=$code")
                input.read(); val atyp = input.read()
                val size = when (atyp) { 1 -> 4; 4 -> 16; 3 -> input.read(); else -> error("bad atyp=$atyp") }
                repeat(size + 2) { check(input.read() >= 0) }
                check(code == 0) { "SOCKS rejected code=$code" }
                stage = "tls_handshake"
                // Layer TLS over the established Tor socket. Host is used for SNI and
                // certificate verification, never to open a separate target socket.
                ((SSLSocketFactory.getDefault() as SSLSocketFactory).createSocket(socket, host, 443, true) as SSLSocket).use { tls ->
                    tls.sslParameters = tls.sslParameters.apply { endpointIdentificationAlgorithm = "HTTPS" }
                    tls.soTimeout = 90000
                    tls.startHandshake()
                    emit("TLS_PASS,$label,protocol=${tls.session.protocol}")
                    stage = "http_response"
                    tls.getOutputStream().apply {
                        write("GET $path HTTP/1.1\r\nHost: $host\r\nConnection: close\r\n\r\n".toByteArray()); flush()
                    }
                    val line = tls.getInputStream().bufferedReader().readLine() ?: error("HTTP EOF")
                    val status = line.split(' ').getOrNull(1)?.toIntOrNull()
                    check(status == 200 || status in listOf(301, 302, 307, 308)) { "HTTP status=$status" }
                    emit("EXTERNAL_PASS,$label,socks=$code,http=$status")
                }
            }
        } catch (t: Throwable) {
            emit("EXTERNAL_FAILURE,$label,port=$port,stage=$stage,socks=$code,type=${t.javaClass.name},message=${t.message},current=${tor.status.value},hasClient=${tor.hasClient}")
        }
    }

    suspend fun run(localOnly: Boolean = false) = coroutineScope {
        val tor = ArtiTorClient()
        emit("RELIABILITY_VERSION,${tor.version}")
        val jobs = mutableListOf<Job>()
        jobs += launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) { tor.status.collect {
            history += "${System.nanoTime()}:root:$it"
        } }
        jobs += launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) { tor.logs.collect { emit("DIAGNOSTIC_PUBLIC_LOG,$it") } }
        fun observe(s: TorIsolationSession, label: String) {
            jobs += launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) { s.status.collect {
                history += "${System.nanoTime()}:$label:$it"
                check((it.socksEndpoint != null) == (it.state == TorIsolationSessionState.ACTIVE))
            } }
        }
        val config = ArtiConfig(File(context.filesDir, "arti-reliability").apply { mkdirs() }.absolutePath)
        val markers = listOf("192.0.2.231", "F00DF00DF00DF00DF00DF00DF00DF00DF00DF00D", "SYNTHETIC_RELIABILITY_TRANSPORT_SECRET")
        try {
            // Synthetic invalid config is exercised in the first process, before bootstrap.
            val failure = tor.start(config.copy(bridges = listOf("${markers[2]} ${markers[0]}:443 ${markers[1]} cert=FAKE invalid"), bridgesEnabled = BridgesEnabled.ON)).exceptionOrNull()
            check(failure is ArtiException.Config)
            check(markers.none { failure.toString().contains(it) })
            emit("REDACTION_EXCEPTION_PASS,kind=${failure.kind},exception=$failure")
            tor.start(config, 180.seconds).getOrThrow()
            greeting(root(tor), "cold_1"); roots++; colds++
            repeat(49) { greeting(root(tor), "root_observation_${it + 1}"); roots++ }
            emit("ROOT_INITIAL_SUMMARY,pass=$roots")
            repeat(100) { i ->
                val s = tor.createIsolationSession().getOrThrow()
                observe(s, "create_$i")
                greeting(active(s), "session_create_$i"); sessions++
                s.close()
            }
            emit("SESSION_CREATE_SUMMARY,pass=$sessions")
            val retained = tor.createIsolationSession().getOrThrow(); observe(retained, "retained")
            repeat(100) { i ->
                val oldRoot = root(tor); val oldSession = active(retained)
                val pauseBegin = System.nanoTime()
                tor.pause()
                val pauseAt = System.nanoTime()
                emit("PAUSE_TIMING,cycle=$i,duration_ns=${pauseAt - pauseBegin}")
                check(tor.hasClient && tor.status.value.state == TorState.PAUSED)
                check(retained.status.value.state == TorIsolationSessionState.PAUSED)
                refused(oldRoot, "pause_root_$i", tor, retained, pauseAt)
                refused(oldSession, "pause_session_$i", tor, retained, pauseAt)
                tor.resume(45.seconds).getOrThrow()
                greeting(root(tor), "resume_root_$i"); roots++
                greeting(active(retained), "resume_session_$i"); resumes++
            }
            emit("RESUME_SUMMARY,pass=$resumes,root_pass=$roots,old_refused=200")
            retained.close()
            repeat(2) { i ->
                tor.shutdown(); check(!tor.hasClient)
                tor.start(config, 180.seconds).getOrThrow()
                greeting(root(tor), "cold_${i + 2}"); roots++; colds++
            }
            emit("LOCAL_SUMMARY,root=$roots,session_create=$sessions,resume=$resumes,cold=$colds,failures=0")
            // Two fixed attempts per existing repository target; no retries or extra circuits.
            if (!localOnly) {
                val port = root(tor)
                for (i in 1..2) {
                    https(tor, port, "check.torproject.org", "/api/ip", "check_$i")
                    https(tor, port, "api.ipify.org", "/", "ipify_$i")
                }
            }
        } finally {
            tor.shutdown(); jobs.forEach { it.cancel() }
        }
    }
}
