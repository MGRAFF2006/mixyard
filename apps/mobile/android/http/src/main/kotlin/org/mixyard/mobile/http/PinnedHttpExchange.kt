package org.mixyard.mobile.http

import java.net.Proxy
import java.net.URI
import java.security.MessageDigest
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager
import okhttp3.Authenticator
import okhttp3.ConnectionPool
import okhttp3.ConnectionSpec
import okhttp3.CookieJar
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.TlsVersion
import okio.BufferedSink

/** Shipping HTTP engine used unchanged on Android and in JVM qualification.
 * No authentication follow-up, redirect, proxy, cookie, cache or POST replay. */
class PinnedHttpExchange(origin: URI, pin: String, path: String, body: String, credential: String?) : AutoCloseable {
    private val limit = 2_097_152
    private val client: OkHttpClient
    private val call: okhttp3.Call
    init {
        require(origin.scheme == "https" && !origin.host.isNullOrBlank() && origin.rawUserInfo == null && origin.rawQuery == null && origin.rawFragment == null && origin.rawPath in listOf("", "/")) { "exact-https-origin-required" }
        require(path in setOf("/control/pair", "/control/session", "/control/request", "/control/credentials", "/control/rotate", "/control/revoke")) { "path-denied" }
        require(pin.matches(Regex("[a-fA-F0-9]{64}"))) { "leaf-sha256-required" }
        val expected = pin.lowercase()
        val system = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply {
            init(null as java.security.KeyStore?)
        }.trustManagers.filterIsInstance<X509TrustManager>().single()
        val trust = object : X509TrustManager {
            override fun getAcceptedIssuers(): Array<X509Certificate> = system.acceptedIssuers
            override fun checkClientTrusted(chain: Array<X509Certificate>, type: String) = system.checkClientTrusted(chain, type)
            override fun checkServerTrusted(chain: Array<X509Certificate>, type: String) {
                system.checkServerTrusted(chain, type)
                val actual = MessageDigest.getInstance("SHA-256").digest(chain.first().encoded).joinToString("") { "%02x".format(it) }
                if (actual != expected) throw java.security.cert.CertificateException("certificate-pin-mismatch")
            }
        }
        val ssl = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), null) }
        client = OkHttpClient.Builder()
            .sslSocketFactory(ssl.socketFactory, trust)
            .connectionSpecs(listOf(ConnectionSpec.Builder(ConnectionSpec.MODERN_TLS).tlsVersions(TlsVersion.TLS_1_3).build()))
            .protocols(listOf(Protocol.HTTP_1_1))
            .retryOnConnectionFailure(false).followRedirects(false).followSslRedirects(false)
            .authenticator(Authenticator.NONE).proxyAuthenticator(Authenticator.NONE).proxy(Proxy.NO_PROXY)
            .cookieJar(CookieJar.NO_COOKIES).cache(null)
            .connectionPool(ConnectionPool(0, 1, TimeUnit.SECONDS))
            .connectTimeout(5, TimeUnit.SECONDS).readTimeout(8, TimeUnit.SECONDS)
            .writeTimeout(8, TimeUnit.SECONDS).callTimeout(12, TimeUnit.SECONDS).build()
        val encoded = body.toByteArray(Charsets.UTF_8)
        require(encoded.size <= limit) { "request-too-large" }
        val once = object : RequestBody() {
            override fun contentType() = "application/json".toMediaType()
            override fun contentLength() = encoded.size.toLong()
            override fun isOneShot() = true
            override fun writeTo(sink: BufferedSink) { sink.write(encoded) }
        }
        val request = Request.Builder().url(origin.resolve(path).toString()).post(once)
            .header("Accept-Encoding", "identity")
        if (credential != null) request.header("Authorization", "Bearer $credential")
        call = client.newCall(request.build())
    }
    fun cancel() { call.cancel() }
    fun execute(): Pair<Int, String> = call.execute().use { response ->
        require(response.code !in 300..399) { "redirect-denied" }
        require(response.header("Content-Encoding") in listOf(null, "identity")) { "unexpected-content-encoding" }
        val body = response.body ?: error("empty-control-error")
        require(body.contentLength() <= limit) { "response-too-large" }
        val bytes = body.byteStream().use { stream ->
            val result = java.io.ByteArrayOutputStream()
            val chunk = ByteArray(8192)
            while (true) {
                val count = stream.read(chunk)
                if (count < 0) break
                require(count <= limit - result.size()) { "response-too-large" }
                result.write(chunk, 0, count)
            }
            result.toByteArray()
        }
        response.code to Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(bytes)).toString()
    }
    override fun close() { call.cancel(); client.connectionPool.evictAll(); client.dispatcher.executorService.shutdown() }
}
