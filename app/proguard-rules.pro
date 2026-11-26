# JNI entry points
-keep class com.emucam.nativebridge.EmuNative { *; }
# kotlinx.serialization
-keepattributes *Annotation*, InnerClasses
-keepclassmembers class com.emucam.** { *** Companion; kotlinx.serialization.KSerializer serializer(...); }
