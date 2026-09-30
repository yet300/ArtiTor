package com.yet.tor

import com.yet.tor.ffi.ArtiErrorDetail as FfiErrorDetail
import com.yet.tor.ffi.ErrorKind as FfiErrorKind
import com.yet.tor.ffi.TorErrorKind as FfiTorErrorKind
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

class ErrorClassificationTest {
    @Test fun legacyConstructorsKeepDefaultKinds() {
        val expected = listOf(
            ArtiException.AlreadyRunning() to TorErrorKind.ALREADY_RUNNING,
            ArtiException.NotRunning() to TorErrorKind.NOT_RUNNING,
            ArtiException.Config("invalid") to TorErrorKind.CONFIG,
            ArtiException.Bind(9050, "occupied") to TorErrorKind.BIND,
            ArtiException.Bootstrap("failed") to TorErrorKind.BOOTSTRAP,
            ArtiException.Timeout() to TorErrorKind.TIMEOUT,
            ArtiException.Runtime("failed") to TorErrorKind.RUNTIME,
        )
        expected.forEach { (exception, kind) -> assertEquals(kind, exception.kind) }
    }

    @Test fun classificationIsIndependentOfMisleadingMessageAndOuterClass() {
        for (category in FfiTorErrorKind.entries) {
            val expected = TorErrorKind.valueOf(category.name)
            for (text in listOf("bootstrap failed", "bind port 9050", "invalid target", "unrelated words", "")) {
                val runtime = FfiErrorDetail(FfiErrorKind.RUNTIME, category, null, text).toPublic()
                assertIs<ArtiException.Runtime>(runtime)
                assertEquals(expected, runtime.kind)
                val bootstrap = FfiErrorDetail(FfiErrorKind.BOOTSTRAP, category, null, text).toPublic()
                assertIs<ArtiException.Bootstrap>(bootstrap)
                assertEquals(expected, bootstrap.kind)
            }
        }
    }

    @Test fun synchronousTransportKeepsClassAndCategory() {
        for (category in FfiTorErrorKind.entries) {
            val expected = TorErrorKind.valueOf(category.name)
            val runtime = com.yet.tor.ffi.ArtiException.Runtime("misleading bootstrap text", category).toPublic()
            assertIs<ArtiException.Runtime>(runtime)
            assertEquals(expected, runtime.kind)
            val bootstrap = com.yet.tor.ffi.ArtiException.Bootstrap("misleading bind text", category).toPublic()
            assertIs<ArtiException.Bootstrap>(bootstrap)
            assertEquals(expected, bootstrap.kind)
        }
    }

    @Test fun terminalCategoriesUseExistingRuntimeClass() {
        for (kind in listOf(TorErrorKind.SESSION_CLOSED, TorErrorKind.SESSION_INVALIDATED)) {
            val exception = ArtiException.Runtime("session no longer usable", kind)
            assertEquals(kind, exception.kind)
            assertIs<ArtiException.Runtime>(exception)
        }
    }

    @Test fun unknownRemainsUnknown() {
        assertEquals(TorErrorKind.UNKNOWN,
            FfiErrorDetail(FfiErrorKind.RUNTIME, FfiTorErrorKind.UNKNOWN, null, "known-looking runtime failure").toPublic().kind)
    }
}
