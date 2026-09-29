package com.yet.tor

import com.yet.tor.ffi.ArtiErrorDetail as FfiErrorDetail
import com.yet.tor.ffi.ErrorKind as FfiErrorKind
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

class ErrorMappingTest {

    @Test
    fun bindDetailMapsToBindWithPort() {
        val e = FfiErrorDetail(FfiErrorKind.BIND, 9060u, "address in use").toPublic()
        assertIs<ArtiException.Bind>(e)
        assertEquals(9060, e.port)
    }

    @Test
    fun bootstrapDetailMapsToBootstrap() {
        val e = FfiErrorDetail(FfiErrorKind.BOOTSTRAP, null, "no consensus").toPublic()
        assertIs<ArtiException.Bootstrap>(e)
    }

    @Test
    fun configDetailMapsToConfig() {
        val e = FfiErrorDetail(FfiErrorKind.CONFIG, null, "bad bridge line").toPublic()
        assertIs<ArtiException.Config>(e)
    }

    @Test
    fun runtimeDetailMapsToRuntime() {
        val e = FfiErrorDetail(FfiErrorKind.RUNTIME, null, "boom").toPublic()
        assertIs<ArtiException.Runtime>(e)
    }

    @Test
    fun alreadyRunningAndNotRunningMap() {
        assertIs<ArtiException.AlreadyRunning>(
            FfiErrorDetail(FfiErrorKind.ALREADY_RUNNING, null, "x").toPublic(),
        )
        assertIs<ArtiException.NotRunning>(
            FfiErrorDetail(FfiErrorKind.NOT_RUNNING, null, "x").toPublic(),
        )
    }

    @Test
    fun kindDrivesTypeNeverMessageText() {
        // Adversarial: a Bootstrap message containing bind-like text must NOT
        // become Bind; a Bind message containing bootstrap-like text must NOT
        // become Bootstrap. Proves no string parsing.
        val bootstrap = FfiErrorDetail(
            FfiErrorKind.BOOTSTRAP,
            null,
            "failed to bind SOCKS on 9050: address in use",
        ).toPublic()
        assertIs<ArtiException.Bootstrap>(bootstrap)

        val bind = FfiErrorDetail(
            FfiErrorKind.BIND,
            9050u,
            "bootstrap failed: no consensus",
        ).toPublic()
        val asBind = assertIs<ArtiException.Bind>(bind)
        assertEquals(9050, asBind.port)
        assertTrue(asBind.message!!.contains("bootstrap failed"))
    }
}
