#!/usr/bin/env python3
"""Generated-fixture contract harness; Python is not part of the product."""
import pathlib, shutil, subprocess, sys, tempfile
exe = pathlib.Path(sys.argv[1]).resolve()
def run(*a, ok=True):
    p = subprocess.run([exe, *map(str,a)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert (p.returncode == 0) == ok, p.stderr.decode()
def case(data):
    d=pathlib.Path(tempfile.mkdtemp()); src=d/"in"; repo=d/"repo"; out=d/"out"; src.write_bytes(data)
    run("pack",src,repo,17); run("restore",repo,out,268435456); assert out.read_bytes()==data
    for kind in ("corrupt","truncate","missing","extra","manifest","sidecar","badrows"):
        q=d/(kind+"repo"); shutil.copytree(repo,q)
        if kind=="corrupt": (q/"chunk-0000000000000000.cix").write_bytes(b"x")
        elif kind=="truncate": (q/"chunk-0000000000000000.cix").write_bytes(b"")
        elif kind=="missing": (q/"chunk-0000000000000000.cix").unlink()
        elif kind=="extra": (q/"extra").write_bytes(b"x")
        elif kind=="manifest": (q/"manifest.cixb1").write_bytes(b"changed\n")
        elif kind=="sidecar": (q/"manifest.cixb1.sha256").write_bytes(b"0 " + b"0"*64 + b"\n")
        else:
            m=q/"manifest.cixb1"; text=m.read_text().splitlines(); text[1]="999999 1 17 " + text[1].split()[3]; raw=("\n".join(text)+"\n").encode(); m.write_bytes(raw)
            import hashlib; (q/"manifest.cixb1.sha256").write_text(f"{len(raw)} {hashlib.sha256(raw).hexdigest()}\n")
        run("restore",q,d/(kind+"out"),268435456,ok=False)
    run("restore",repo,d/"wrongcap",max(0,len(data)-1),ok=not data); run("pack",src,repo,17,ok=False); run("restore",repo,out,len(data),ok=False); shutil.rmtree(d)
case(b""); case(bytes(range(256))*3)
print("storage fixture contract passed")
