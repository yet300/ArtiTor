# Consumer ProGuard/R8 rules shipped inside the AAR — applied automatically to any
# app that depends on this library. The Gobley/UniFFI Android bindings call into the
# native library via JNA and receive native->Kotlin callbacks (StatusListener), both
# of which rely on reflection and must survive minification.

# JNA core (loaded reflectively; references java.awt which is absent on Android).
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }
-keep class * implements com.sun.jna.Library { *; }
-keep class * implements com.sun.jna.Callback { *; }

# UniFFI-generated bindings: JNA Structure subclasses (field order/names read by JNA)
# and callback-interface dispatchers invoked from native code.
-keep class com.yet.tor.ffi.** { *; }
-keepclassmembers class com.yet.tor.ffi.** { *; }
