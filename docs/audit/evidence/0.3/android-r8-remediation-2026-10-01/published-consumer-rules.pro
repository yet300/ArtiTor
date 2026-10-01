# Consumer ProGuard/R8 rules shipped inside the AAR — applied automatically to any
# app that depends on this library. The Ubique/UniFFI Android backend calls into the
# native library via JNA and receives native->Kotlin callbacks (StatusListener), both
# of which rely on reflection and must survive minification.

# JNA core (loaded reflectively; references java.awt which is absent on Android).
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }
-keep class * implements com.sun.jna.Library { *; }
-keep class * implements com.sun.jna.Callback { *; }

# `@Structure.FieldOrder` is read reflectively when the struct layout is built.
-keepattributes RuntimeVisibleAnnotations

# R8 full mode retains class annotations only on classes matched by a keep rule.
# Ubique's runtime structs live outside com.yet.tor.ffi; member-only JNA rules
# preserve their fields but not @Structure.FieldOrder. Keep the class metadata
# eligible while allowing unused structs to shrink and names to be obfuscated.
# The JNA member rule above preserves reflected fields and constructors.
-keep,allowshrinking,allowoptimization,allowobfuscation class uniffi.runtime.** extends com.sun.jna.Structure

# UniFFI-generated bindings: JNA Structure subclasses (field order/names read by JNA)
# and callback-interface dispatchers invoked from native code.
-keep class com.yet.tor.ffi.** { *; }
-keepclassmembers class com.yet.tor.ffi.** { *; }
