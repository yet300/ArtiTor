package consumer.artitor03.app

import android.app.Activity
import android.os.Bundle
import android.os.Debug
import android.util.Log
import android.widget.TextView
import com.yet.tor.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import java.io.File
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.Collections
import kotlin.time.Duration.Companion.seconds

class MainActivity : Activity() {
    private val tag = "ArtiGate"
    private val output = StringBuilder()
    private lateinit var evidence: File
    private fun emit(s: String) = synchronized(output) {
        output.appendLine(s); evidence.writeText(output.toString()); Log.i(tag, s)
    }
    private fun ok(label: String, condition: Boolean) { check(condition) { label }; emit("PASS,$label") }
    private fun resources(label: String, tor: ArtiTorClient) {
        val status = File("/proc/self/status").readLines()
        val rss = status.firstOrNull { it.startsWith("VmRSS:") }
        emit("RESOURCE,$label,sessions=${tor.sessions.size},listeners=${tor.sessions.count { it.status.value.socksEndpoint != null } + if(tor.socksEndpoint != null) 1 else 0},fd=${File("/proc/self/fd").list()!!.size},threads=${File("/proc/self/task").list()!!.size},$rss,pss_kb=${Debug.getPss()}")
    }
    private fun dead(port: Int, label: String) {
        val connected = runCatching { Socket().use { it.connect(InetSocketAddress("127.0.0.1", port), 1500) } }.isSuccess
        ok("listener_dead,$label", !connected)
    }
    // Only opens a loopback SOCKS socket. Target names are sent to Tor; no direct DNS or fallback path exists.
    private fun request(port: Int, host: String, label: String, rejected: Boolean = false) {
        emit("REQUEST,$label,proxy_port=$port")
        Socket().use { s ->
            s.connect(InetSocketAddress("127.0.0.1", port), 5000); s.soTimeout = 90000
            val input = s.getInputStream(); val out = s.getOutputStream()
            out.write(byteArrayOf(5, 1, 0)); out.flush()
            check(input.read() == 5 && input.read() == 0) { "SOCKS negotiation" }
            val name = host.toByteArray(Charsets.US_ASCII)
            out.write(byteArrayOf(5, 1, 0, 3, name.size.toByte()) + name + byteArrayOf(0, 80)); out.flush()
            check(input.read() == 5); val code = input.read(); input.read(); val atyp = input.read()
            val size = when(atyp) { 1 -> 4; 4 -> 16; 3 -> input.read(); else -> error("SOCKS atyp=$atyp") }
            repeat(size + 2) { check(input.read() >= 0) }
            if (rejected) { ok("SOCKS_rejected,$label,code=$code", code == 5); return }
            check(code == 0) { "SOCKS code $code at $label" }
            out.write("GET / HTTP/1.1\r\nHost: $host\r\nConnection: close\r\n\r\n".toByteArray()); out.flush()
            val line = input.bufferedReader().readLine() ?: error("empty HTTP response at $label")
            val httpCode = line.split(' ').getOrNull(1)?.toIntOrNull()
            ok("HTTP,$label,status=$httpCode", httpCode == 200 || httpCode in listOf(301,302,307,308))
        }
    }
    override fun onCreate(state: Bundle?) {
        super.onCreate(state); setContentView(TextView(this).apply { text = "Running ArtiTor release hardware gate" })
        evidence = File(getExternalFilesDir(null), "gate-${intent.getStringExtra("run") ?: "first"}.txt")
        CoroutineScope(Dispatchers.IO).launch {
            try { gate(); emit("GATE_COMPLETED,PASS") }
            catch(t: Throwable) { emit("GATE_FAILED,${t.javaClass.name},${t.message}\n${t.stackTraceToString()}") }
        }
    }
    private suspend fun gate() = coroutineScope {
        val tor = ArtiTorClient()
        emit("JNA_VERSION,${tor.version}")
        val history = Collections.synchronizedList(mutableListOf<TorState>())
        val logLines = Collections.synchronizedList(mutableListOf<String>())
        val statusJob = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) { tor.status.collect {
            history += it.state; emit("STATUS,${it.state},bootstrap=${it.bootstrapPercent},port=${it.socksPort},kind=${it.lastError?.kind}")
        } }
        val logsJob = launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) { tor.logs.collect {
            logLines += it; emit("PUBLIC_LOG,$it")
        } }
        val sessionJobs = mutableListOf<Job>()
        val sessionHistory = Collections.synchronizedList(mutableListOf<String>())
        fun observe(s: TorIsolationSession, label: String) { sessionJobs += launch(Dispatchers.Unconfined, start = CoroutineStart.UNDISPATCHED) { s.status.collect {
            check((it.socksEndpoint != null) == (it.state == TorIsolationSessionState.ACTIVE))
            sessionHistory += "$label:${it.state}"; emit("SESSION,$label,${it.state},endpoint=${it.socksEndpoint}")
        } } }
        val config = ArtiConfig(File(filesDir, "arti-gate").apply { mkdirs() }.absolutePath)
        val onion = "hjirlp6fu47kox4cnede4zlvaeq672bibss3oxgmsnsc5mdxygqshbqd.onion"
        fun port(s: TorIsolationSession) = requireNotNull(s.status.value.socksEndpoint).port
        fun root() = requireNotNull(tor.socksEndpoint).port
        val markers = listOf("192.0.2.123", "ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEF", "SYNTHETIC_SECRET_GATE")
        try {
            resources("process_before_start", tor)
            val cold = System.nanoTime(); tor.start(config, 180.seconds).getOrThrow()
            emit("TIMING,cold_ms=${(System.nanoTime()-cold)/1000000}")
            ok("bootstrap100", tor.status.value.bootstrapPercent == 100)
            request(root(), "api.ipify.org", "root_first_cold")
            resources("loaded_root_only", tor)
            val a = tor.createIsolationSession().getOrThrow(); observe(a, "A")
            val b = tor.createIsolationSession().getOrThrow(); observe(b, "B")
            ok("distinct_root_A_B", setOf(root(), port(a), port(b)).size == 3)
            request(port(a), "api.ipify.org", "A_first_cold")
            request(port(b), "api.ipify.org", "B_first_cold")
            val oldA = port(a); a.close(); delay(100)
            ok("A_CLOSED", a.status.value.state == TorIsolationSessionState.CLOSED && a.status.value.socksEndpoint == null)
            dead(oldA, "closed_A")
            request(root(), "api.ipify.org", "root_after_A_close")
            request(port(b), "api.ipify.org", "B_after_A_close")
            resources("loaded_root_plus_1", tor)
            val extra = mutableListOf<TorIsolationSession>()
            repeat(7) { extra += tor.createIsolationSession().getOrThrow() }
            // One additional used session plus B; six idle listeners. Avoid creating eight traffic circuits.
            request(port(extra.first()), "api.ipify.org", "capacity_used_extra")
            resources("loaded_root_plus_8_two_used", tor)
            extra.forEach { it.close() }; delay(1000)
            resources("root_plus_1_used", tor)
            val idle = mutableListOf(b)
            for(count in listOf(1,8,16,32)) {
                while(idle.size < count) idle += tor.createIsolationSession().getOrThrow()
                ok("unique_endpoints_$count", (idle.map { port(it) } + root()).toSet().size == count + 1)
                delay(3000); repeat(3) { resources("listeners_${count}_B_used", tor); delay(500) }
            }
            val cap = tor.createIsolationSession().exceptionOrNull()
            ok("33rd_public_Runtime", cap is ArtiException.Runtime && cap.kind == TorErrorKind.RUNTIME)
            val idlePorts = idle.drop(1).map { port(it) }; idle.drop(1).forEach { it.close() }; delay(1000)
            idlePorts.forEach { dead(it,"idle_closed") }; resources("after_31_close_B_retained", tor)
            val live = b
            request(port(live),"api.ipify.org","remaining_before_pause")
            val oldRoot=root(); val oldLive=port(live); val pauseAt=System.nanoTime()
            tor.pause(); delay(1000)
            ok("pause_retains_client", tor.hasClient && tor.status.value.state == TorState.PAUSED && tor.socksEndpoint == null)
            ok("session_PAUSED",live.status.value.state == TorIsolationSessionState.PAUSED && live.status.value.socksEndpoint == null)
            dead(oldRoot,"paused_root"); dead(oldLive,"paused_session")
            emit("TIMING,pause_ms=${(System.nanoTime()-pauseAt)/1000000}")
            val bootstrapBefore=history.count { it == TorState.BOOTSTRAPPING }
            val warm=System.nanoTime(); tor.resume(45.seconds).getOrThrow()
            emit("TIMING,resume_ms=${(System.nanoTime()-warm)/1000000}")
            ok("resume_root_ready",tor.status.value.state==TorState.RUNNING && tor.isReady)
            withTimeout(30_000) { live.status.first { it.state==TorIsolationSessionState.ACTIVE && it.socksEndpoint!=null } }
            ok("resume_no_bootstrap", history.count { it == TorState.BOOTSTRAPPING } == bootstrapBefore && tor.hasClient)
            ok("resume_ACTIVE",live.status.value.state == TorIsolationSessionState.ACTIVE)
            request(port(live),"api.ipify.org","remaining_after_resume")
            request(port(live),onion,"onion_positive")
            val rootSnapshot=tor.status.value; val sessionSnapshot=live.status.value
            for(mode in BridgesEnabled.entries) {
                val failure=tor.start(config.copy(bridges=listOf("obfs4 ${markers[0]}:443 ${markers[1]} cert=${markers[2]} invalid"),bridgesEnabled=mode)).exceptionOrNull()
                ok("malformed_Config_$mode",failure is ArtiException.Config && failure.kind==TorErrorKind.CONFIG)
                check(markers.none { failure.toString().contains(it) })
                ok("replacement_preserved_$mode", tor.status.value==rootSnapshot && live.status.value==sessionSnapshot)
            }
            val empty=tor.start(config.copy(bridgesEnabled=BridgesEnabled.ON,bridges=emptyList())).exceptionOrNull()
            ok("ON_empty_Config",empty is ArtiException.Config)
            request(root(),"api.ipify.org","root_after_invalid_configs")
            request(port(live),"api.ipify.org","session_after_invalid_configs")
            var logStart=logLines.size
            request(root(),"bad.onion","malformed_onion",true); delay(300)
            ok("malformed_onion_TargetRejected",logLines.drop(logStart).any { it.contains("arti_kind=InvalidStreamTarget") })
            val endRoot=root(); val endSession=port(live)
            resources("before_shutdown",tor); tor.shutdown(); delay(1000)
            ok("shutdown_no_client",!tor.hasClient && tor.socksEndpoint==null && tor.sessions.isEmpty())
            ok("shutdown_INVALIDATED",live.status.value.state==TorIsolationSessionState.INVALIDATED && live.status.value.socksEndpoint==null)
            dead(endRoot,"shutdown_root"); dead(endSession,"shutdown_session"); resources("after_shutdown",tor)
            val restart=System.nanoTime(); tor.start(config,180.seconds).getOrThrow()
            emit("TIMING,restart_ms=${(System.nanoTime()-restart)/1000000}")
            val fresh=tor.createIsolationSession().getOrThrow(); observe(fresh,"fresh")
            live.close(); a.close()
            ok("old_handles_terminal",live.status.value.state==TorIsolationSessionState.INVALIDATED && a.status.value.state==TorIsolationSessionState.CLOSED)
            request(root(),"api.ipify.org","fresh_root_restart")
            request(port(fresh),"api.ipify.org","fresh_session_restart")
            tor.start(config.copy(allowOnionAddrs=false),180.seconds).getOrThrow()
            ok("policy_rebuild_invalidates",fresh.status.value.state==TorIsolationSessionState.INVALIDATED)
            logStart=logLines.size; request(root(),onion,"disabled_onion",true); delay(300)
            ok("disabled_onion_TargetRejected",logLines.drop(logStart).any { it.contains("arti_kind=ForbiddenStreamTarget") })
            request(root(),"api.ipify.org","clearnet_after_policy")
            // A local occupied port safely induces a real native asynchronous bind error.
            val errorSession=tor.createIsolationSession().getOrThrow(); observe(errorSession,"error_session")
            tor.pause()
            ServerSocket(0,1,java.net.InetAddress.getByName("127.0.0.1")).use { occupied ->
                val failure=tor.start(config.copy(allowOnionAddrs=false,socksPort=occupied.localPort),45.seconds).exceptionOrNull()
                delay(300)
                ok("async_error_public_Bind",failure is ArtiException.Bind && failure.kind==TorErrorKind.BIND)
                ok("async_error_StateFlow_Bind",tor.status.value.state==TorState.ERROR && tor.status.value.lastError is ArtiException.Bind && tor.status.value.lastError?.kind==TorErrorKind.BIND)
                ok("async_error_session_demoted",errorSession.status.value.state==TorIsolationSessionState.PAUSED && errorSession.status.value.socksEndpoint==null)
            }
            tor.shutdown(); delay(1000); resources("final_shutdown",tor)
            ok("async_error_session_invalidated",errorSession.status.value.state==TorIsolationSessionState.INVALIDATED && errorSession.status.value.socksEndpoint==null)
            ok("status_history",history.containsAll(listOf(TorState.STARTING,TorState.BOOTSTRAPPING,TorState.RUNNING,TorState.PAUSED,TorState.ERROR)))
            ok("session_history",sessionHistory.containsAll(listOf("A:ACTIVE","A:CLOSED","B:PAUSED","B:ACTIVE","B:INVALIDATED")))
            ok("public_logs_callback",logLines.isNotEmpty())
            ok("bridge_redacted",logLines.none { line -> markers.any { line.contains(it) } })
            emit("NATIVE_MAPS,"+File("/proc/self/maps").readLines().filter { it.contains("libarti_kmp_ffi") }.joinToString("|"))
        } finally {
            tor.shutdown(); delay(300); statusJob.cancel(); logsJob.cancel(); sessionJobs.forEach { it.cancel() }
        }
    }
}
