#!/usr/bin/env python3
"""Verify JNA FieldOrder annotations and reflected fields in a minified consumer APK.

Uses original dependency AARs as the annotation inventory and the consumer's R8
mapping to locate surviving classes. Requires JDK javap and SDK dexdump; no pip
dependencies. Unused classes may shrink, but every surviving annotated struct
must retain its original runtime field order and field names. Construction's
RustBufferStruct and UniffiRustCallStatusStruct are mandatory.
"""
import argparse
import json
import pathlib
import re
import subprocess
import tempfile
import zipfile


def verify(apk, mapping, aars, dexdump):
    names = {}
    for line in mapping.read_text().splitlines():
        match = re.fullmatch(r"(\S+) -> (\S+):", line)
        if match:
            names[match[1]] = match[2]
    inventory = {}
    classes = {}
    with tempfile.TemporaryDirectory(prefix="artitor-dex-") as directory:
        directory = pathlib.Path(directory)
        for index, aar in enumerate(aars):
            with zipfile.ZipFile(aar) as archive:
                jar = directory / f"original-{index}.jar"
                jar.write_bytes(archive.read("classes.jar"))
            with zipfile.ZipFile(jar) as archive:
                candidates = [name[:-6].replace("/", ".") for name in archive.namelist()
                              if name.endswith(".class") and
                              b"com/sun/jna/Structure$FieldOrder" in archive.read(name)]
            for name in candidates:
                original = subprocess.check_output(
                    ["javap", "-v", "-classpath", str(jar), name], text=True)
                annotation = re.search(
                    r"com\.sun\.jna\.Structure\$FieldOrder\(\s+value=\[(.*?)\]", original)
                if annotation:
                    inventory[name] = re.findall(r'"([^"]+)"', annotation[1])
        with zipfile.ZipFile(apk) as archive:
            for name in archive.namelist():
                if not re.fullmatch(r"classes\d*\.dex", name):
                    continue
                dex = directory / name
                dex.write_bytes(archive.read(name))
                dump = subprocess.check_output([str(dexdump), "-a", str(dex)], text=True)
                annotations = dict(re.findall(
                    r"Class #(\d+) annotations:\n(.*?)(?=\nClass #)", dump, re.S))
                for match in re.finditer(
                        r"Class #(\d+)\s+-\n(.*?)(?=\nClass #|\Z)", dump, re.S):
                    body = match[2]
                    descriptor = re.search(r"Class descriptor\s+: 'L([^']+);'", body)
                    if descriptor:
                        classes[descriptor[1].replace("/", ".")] = (
                            annotations.get(match[1], ""), body)
    required = {"uniffi.runtime.RustBufferStruct", "uniffi.runtime.UniffiRustCallStatusStruct"}
    failures = []
    checked = []
    for original, fields in sorted(inventory.items()):
        mapped = names.get(original, original)
        if mapped not in classes:
            if original in required:
                failures.append(f"{original}: required construction struct absent from DEX")
            continue
        annotation, body = classes[mapped]
        order = re.search(
            r"VISIBILITY_RUNTIME Lcom/sun/jna/Structure\$FieldOrder; value=\{([^}]+)\}", annotation)
        actual = re.findall(r'"([^"]+)"', order[1]) if order else None
        instance = body.split("Instance fields", 1)[-1].split("Direct methods", 1)[0]
        actual_fields = re.findall(r"name\s+: '([^']+)'", instance)
        superclass = re.search(r"Superclass\s+: '([^']+)'", body)
        checked.append({"original": original, "mapped": mapped, "field_order": actual,
                        "fields": actual_fields, "superclass": superclass[1] if superclass else None})
        if actual != fields:
            failures.append(f"{original} ({mapped}): runtime FieldOrder {actual!r}, expected {fields!r}")
        if not set(fields).issubset(actual_fields):
            failures.append(f"{original}: reflected fields missing/renamed: {actual_fields!r}")
        if original in required and (not superclass or superclass[1] != "Lcom/sun/jna/Structure;"):
            failures.append(f"{original}: no longer directly extends JNA Structure")
    for original in required - inventory.keys():
        failures.append(f"{original}: missing from original AAR annotation inventory")
    return {"apk": str(apk), "inventory_count": len(inventory), "checked_count": len(checked),
            "structures": checked, "failures": failures, "pass": not failures}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk", required=True, type=pathlib.Path)
    parser.add_argument("--mapping", required=True, type=pathlib.Path)
    parser.add_argument("--aar", required=True, action="append", type=pathlib.Path)
    parser.add_argument("--dexdump", required=True, type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    result = verify(args.apk, args.mapping, args.aar, args.dexdump)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    for failure in result["failures"]:
        print("FAIL:", failure)
    print(f"JNA DEX metadata: {'PASS' if result['pass'] else 'FAIL'}; "
          f"{result['checked_count']} surviving / {result['inventory_count']} original annotated structs")
    return 0 if result["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
