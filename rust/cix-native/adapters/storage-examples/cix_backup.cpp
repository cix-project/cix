// SPDX-License-Identifier: MIT
// Linux/POSIX local repository example. A provider adapter must preserve these
// no-follow/no-overwrite and publish-order guarantees.
#include "cix.h"
#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cstdint>
#include <fcntl.h>
#include <filesystem>
#include <iomanip>
#include <iostream>
#include <limits>
#include <memory>
#include <openssl/evp.h>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <vector>
namespace fs = std::filesystem;
constexpr uint64_t CHUNK = 1024 * 1024,
                   ARCHIVE_CAP = CHUNK + 2ULL * 1024 * 1024,
                   MEMORY_BUDGET = 256ULL * 1024 * 1024,
                   MCAP = 16ULL * 1024 * 1024, MAX_ROWS = 65536;
using D = std::array<unsigned char, 32>;
struct R {
  uint64_t i, p, a;
  std::string ph, ah;
};
struct H {
  uint64_t n, t, c;
  std::string wh;
};
[[noreturn]] void bad(const std::string &s) { throw std::runtime_error(s); }
struct Fd {
  int value = -1;
  explicit Fd(int fd = -1) : value(fd) {}
  ~Fd() {
    if (value >= 0)
      close(value);
  }
  Fd(const Fd &) = delete;
  Fd &operator=(const Fd &) = delete;
  int release() {
    int fd = value;
    value = -1;
    return fd;
  }
  void close_checked() {
    int fd = release();
    if (fd >= 0 && close(fd))
      bad("close failed");
  }
};
struct CixContext {
  cix_context *value = nullptr;
  ~CixContext() {
    if (value)
      cix_context_destroy(value);
  }
  CixContext(const CixContext &) = delete;
  CixContext() = default;
};
using MdPtr = std::unique_ptr<EVP_MD_CTX, decltype(&EVP_MD_CTX_free)>;
bool reg(const fs::path &p) {
  struct stat s {};
  return !lstat(p.c_str(), &s) && S_ISREG(s.st_mode);
}
std::string hx(const D &d) {
  std::ostringstream o;
  for (auto b : d)
    o << std::hex << std::setw(2) << std::setfill('0') << unsigned(b);
  return o.str();
}
bool hashstr(const std::string &s) {
  if (s.size() != 64)
    return false;
  for (char c : s)
    if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')))
      return false;
  return true;
}
D digest(const uint8_t *p, size_t n) {
  D d{};
  MdPtr x(EVP_MD_CTX_new(), EVP_MD_CTX_free);
  unsigned z = 0;
  if (!x || EVP_DigestInit_ex(x.get(), EVP_sha256(), 0) != 1 ||
      EVP_DigestUpdate(x.get(), p, n) != 1 ||
      EVP_DigestFinal_ex(x.get(), d.data(), &z) != 1 || z != d.size())
    bad("SHA-256 failure");
  return d;
}
void all(int f, const uint8_t *p, size_t n) {
  while (n) {
    ssize_t w = write(f, p, n);
    if (w < 0 && errno == EINTR)
      continue;
    if (w <= 0)
      bad("write failed");
    p += w;
    n -= size_t(w);
  }
}
std::vector<uint8_t> readfile(const fs::path &p, uint64_t cap) {
  Fd f(open(p.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC));
  struct stat s {};
  if (f.value < 0 || fstat(f.value, &s) || !S_ISREG(s.st_mode) ||
      s.st_size < 0 || uint64_t(s.st_size) > cap)
    bad("unsafe, missing, or oversized object");
  std::vector<uint8_t> v(static_cast<size_t>(s.st_size));
  for (size_t q = 0; q < v.size();) {
    ssize_t n = read(f.value, v.data() + q, v.size() - q);
    if (n < 0 && errno == EINTR)
      continue;
    if (n <= 0)
      bad("truncated object");
    q += size_t(n);
  }
  uint8_t x;
  ssize_t more;
  do {
    more = read(f.value, &x, 1);
  } while (more < 0 && errno == EINTR);
  if (more != 0)
    bad("object changed");
  f.close_checked();
  return v;
}
void put(const fs::path &p, const std::vector<uint8_t> &v) {
  Fd f(open(p.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
            0600));
  if (f.value < 0)
    bad("exclusive create failed");
  try {
    all(f.value, v.data(), v.size());
    if (fsync(f.value))
      bad("write sync failed");
    f.close_checked();
  } catch (...) {
    unlink(p.c_str());
    throw;
  }
}
std::string name(uint64_t i) {
  char b[48];
  snprintf(b, sizeof b, "chunk-%016llu.cix", (unsigned long long)i);
  return b;
}
uint64_t num(const std::string &s) {
  if (s.empty() || (s.size() > 1 && s[0] == '0'))
    bad("non-canonical number");
  uint64_t x = 0;
  for (char c : s) {
    if (c < '0' || c > '9' ||
        x > (std::numeric_limits<uint64_t>::max() - unsigned(c - '0')) / 10)
      bad("bad number");
    x = x * 10 + unsigned(c - '0');
  }
  return x;
}
std::vector<std::string> words(const std::string &s, size_t n) {
  std::istringstream x(s);
  std::vector<std::string> v;
  std::string a;
  while (x >> a)
    v.push_back(a);
  if (v.size() != n)
    bad("malformed manifest");
  return v;
}
std::vector<uint8_t> codec(bool dec, const std::vector<uint8_t> &in,
                           uint64_t output_limit) {
  cix_options_v1 o{};
  CixContext c;
  size_t n = 0, w = 0;
  if (output_limit > MEMORY_BUDGET || cix_options_v1_default(&o))
    bad("invalid CIX output limit");
  o.output_limit = std::max<uint64_t>(1, output_limit);
  o.memory_limit =
      MEMORY_BUDGET - in.size() - output_limit - MCAP - 32ULL * 1024 * 1024;
  o.workers = 1;
  if (cix_context_create(&o, &c.value))
    bad("CIX context failed");
  auto fn = dec ? cix_decode_buffer : cix_encode_buffer;
  auto s =
      fn(c.value, in.empty() ? nullptr : in.data(), in.size(), nullptr, 0, &n);
  if ((s != CIX_STATUS_OUTPUT_TOO_SMALL && s != CIX_STATUS_OK) ||
      n > output_limit)
    bad("CIX output limit exceeded");
  std::vector<uint8_t> out(n);
  s = fn(c.value, in.empty() ? nullptr : in.data(), in.size(),
         out.empty() ? nullptr : out.data(), out.size(), &w);
  if (s || w > out.size())
    bad("CIX operation failed");
  out.resize(w);
  return out;
}
fs::path stage(const fs::path &to) {
  fs::path p = to.parent_path().empty() ? "." : to.parent_path();
  struct stat s {};
  if (lstat(p.c_str(), &s) || !S_ISDIR(s.st_mode))
    bad("bad repository parent");
  for (int i = 0; i < 100; i++) {
    auto q =
        p / (".cix-stage-" + std::to_string(getpid()) + "-" +
             std::to_string(
                 std::chrono::steady_clock::now().time_since_epoch().count()) +
             "-" + std::to_string(i));
    if (!mkdir(q.c_str(), 0700))
      return q;
    if (errno != EEXIST)
      bad("cannot create staging repository");
  }
  bad("cannot create staging repository");
}
void syncdir(const fs::path &p) {
  Fd f(open(p.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC));
  if (f.value < 0 || fsync(f.value)) {
    bad("directory sync failed");
  }
  f.close_checked();
}
void pack(const fs::path &in, const fs::path &repo, uint64_t cs) {
  if (!cs || cs > CHUNK || fs::exists(repo) || fs::is_symlink(repo))
    bad("bad input, chunk size, or existing repository");
  Fd f(open(in.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC));
  struct stat st {};
  if (f.value < 0 || fstat(f.value, &st) || !S_ISREG(st.st_mode) ||
      st.st_size < 0)
    bad("input must be a regular non-symlink file");
  uint64_t total = st.st_size;
  if (total > std::numeric_limits<uint64_t>::max() - (cs - 1))
    bad("input is too large");
  uint64_t rows = total ? (total + cs - 1) / cs : 1;
  if (rows > MAX_ROWS)
    bad("too many manifest rows");
  auto tmp = stage(repo);
  try {
    MdPtr w(EVP_MD_CTX_new(), EVP_MD_CTX_free);
    if (!w || EVP_DigestInit_ex(w.get(), EVP_sha256(), 0) != 1)
      bad("SHA init failed");
    std::vector<R> rs;
    rs.reserve(static_cast<size_t>(rows));
    std::vector<uint8_t> b(static_cast<size_t>(cs));
    for (uint64_t i = 0; i < rows; i++) {
      size_t want = total ? size_t(std::min<uint64_t>(cs, total - i * cs)) : 0,
             got = 0;
      while (got < want) {
        ssize_t z = read(f.value, b.data() + got, want - got);
        if (z < 0 && errno == EINTR)
          continue;
        if (z <= 0)
          bad("input truncated");
        got += size_t(z);
      }
      if (EVP_DigestUpdate(w.get(), b.data(), want) != 1)
        bad("SHA update failed");
      std::vector<uint8_t> p(b.begin(), b.begin() + want),
          a = codec(false, p,
                    std::min<uint64_t>(MEMORY_BUDGET, uint64_t(p.size()) +
                                                          2ULL * 1024 * 1024));
      R r{i, uint64_t(p.size()), uint64_t(a.size()),
          hx(digest(p.data(), p.size())), hx(digest(a.data(), a.size()))};
      rs.push_back(r);
      put(tmp / name(i), a);
    }
    uint8_t x;
    ssize_t more;
    do {
      more = read(f.value, &x, 1);
    } while (more < 0 && errno == EINTR);
    if (more != 0)
      bad("input changed");
    f.close_checked();
    D d{};
    unsigned z = 0;
    if (EVP_DigestFinal_ex(w.get(), d.data(), &z) != 1 || z != d.size())
      bad("SHA final failed");
    std::ostringstream m;
    m << "CIXBACKUP1\n"
      << rows << ' ' << total << ' ' << cs << ' ' << hx(d) << '\n';
    for (auto &r : rs)
      m << r.i << ' ' << r.p << ' ' << r.a << ' ' << r.ph << ' ' << r.ah
        << '\n';
    auto ms = m.str();
    if (ms.size() > MCAP)
      bad("manifest exceeds 16 MiB");
    std::vector<uint8_t> mv(ms.begin(), ms.end());
    put(tmp / "manifest.cixb1", mv);
    auto side = std::to_string(mv.size()) + " " +
                hx(digest(mv.data(), mv.size())) + "\n";
    put(tmp / "manifest.cixb1.sha256",
        std::vector<uint8_t>(side.begin(), side.end()));
    syncdir(tmp);
#ifdef SYS_renameat2
    if (syscall(SYS_renameat2, AT_FDCWD, tmp.c_str(), AT_FDCWD, repo.c_str(),
                1))
      bad("safe repository publish failed");
    syncdir(repo.parent_path().empty() ? "." : repo.parent_path());
#else
    bad("requires Linux renameat2(RENAME_NOREPLACE)");
#endif
  } catch (...) {
    std::error_code e;
    fs::remove_all(tmp, e);
    throw;
  }
}
H manifest(const fs::path &r, std::vector<R> &rs) {
  auto s = readfile(r / "manifest.cixb1.sha256", 256);
  std::string q(s.begin(), s.end());
  if (q.empty() || q.back() != '\n')
    bad("bad sidecar");
  q.pop_back();
  auto sf = words(q, 2);
  uint64_t len = num(sf[0]);
  if (len > MCAP || !hashstr(sf[1]))
    bad("bad sidecar");
  auto v = readfile(r / "manifest.cixb1", len);
  if (v.size() != len || hx(digest(v.data(), v.size())) != sf[1])
    bad("manifest sidecar mismatch");
  std::string m(v.begin(), v.end()), l;
  if (m.empty() || m.back() != '\n')
    bad("truncated manifest");
  std::istringstream x(m);
  if (!getline(x, l) || l != "CIXBACKUP1" || !getline(x, l))
    bad("bad header");
  auto f = words(l, 4);
  H h{num(f[0]), num(f[1]), num(f[2]), f[3]};
  if (!h.n || h.n > MAX_ROWS || !h.c || h.c > CHUNK || !hashstr(h.wh) ||
      h.t > std::numeric_limits<uint64_t>::max() - (h.c - 1))
    bad("bad limits");
  uint64_t expected = h.t ? (h.t + h.c - 1) / h.c : 1;
  if (expected != h.n)
    bad("inconsistent row count");
  rs.reserve(static_cast<size_t>(h.n));
  for (uint64_t i = 0; i < h.n; i++) {
    if (!getline(x, l))
      bad("truncated rows");
    auto a = words(l, 5);
    R z{num(a[0]), num(a[1]), num(a[2]), a[3], a[4]};
    uint64_t expected_plain = (i + 1 < h.n) ? h.c : h.t - h.c * (h.n - 1);
    if (z.i != i || z.p != expected_plain || z.a > ARCHIVE_CAP ||
        !hashstr(z.ph) || !hashstr(z.ah))
      bad("bad row");
    rs.push_back(z);
  }
  if (getline(x, l))
    bad("trailing manifest");
  return h;
}
void restore(const fs::path &r, const fs::path &out, uint64_t cap) {
  if (fs::exists(out) || fs::is_symlink(out))
    bad("existing output");
  struct stat s {};
  if (lstat(r.c_str(), &s) || !S_ISDIR(s.st_mode))
    bad("bad repository");
  std::vector<R> rs;
  H h = manifest(r, rs);
  if (h.t > cap)
    bad("output cap exceeded");
  std::vector<std::string> need{"manifest.cixb1", "manifest.cixb1.sha256"};
  for (auto &z : rs)
    need.push_back(name(z.i));
  std::set<std::string> required(need.begin(), need.end());
  for (auto &e : fs::directory_iterator(r)) {
    if (e.is_symlink() || !e.is_regular_file() ||
        !required.count(e.path().filename().string()))
      bad("extra or unsafe repository object");
  }
  for (auto &n : need)
    if (!reg(r / n))
      bad("missing repository object");
  fs::path p = out.parent_path().empty() ? "." : out.parent_path();
  std::string t = (p / ".cix-restore-XXXXXX").string();
  std::vector<char> b(t.begin(), t.end());
  b.push_back(0);
  Fd fd(mkstemp(b.data()));
  if (fd.value < 0)
    bad("cannot create output temporary");
  try {
    MdPtr w(EVP_MD_CTX_new(), EVP_MD_CTX_free);
    if (!w || EVP_DigestInit_ex(w.get(), EVP_sha256(), 0) != 1)
      bad("SHA init");
    uint64_t sum = 0;
    for (auto &z : rs) {
      auto a = readfile(r / name(z.i), z.a);
      if (a.size() != z.a || hx(digest(a.data(), a.size())) != z.ah)
        bad("archive corruption");
      auto v = codec(true, a, z.p);
      if (v.size() != z.p || hx(digest(v.data(), v.size())) != z.ph)
        bad("plain corruption");
      if (z.p > h.t - sum)
        bad("total overflow");
      sum += z.p;
      if (EVP_DigestUpdate(w.get(), v.data(), v.size()) != 1)
        bad("SHA update");
      all(fd.value, v.data(), v.size());
    }
    D d{};
    unsigned n = 0;
    if (EVP_DigestFinal_ex(w.get(), d.data(), &n) != 1 || sum != h.t ||
        n != d.size() || hx(d) != h.wh)
      bad("whole checksum");
    if (fsync(fd.value)) {
      bad("output sync");
    }
    fd.close_checked();
    if (link(b.data(), out.c_str()))
      bad("output exists or publish failed");
    if (unlink(b.data()))
      bad("temporary cleanup failed");
    syncdir(p);
  } catch (...) {
    unlink(b.data());
    throw;
  }
}
int main(int ac, char **av) {
  try {
    if (ac == 5 && std::string(av[1]) == "pack") {
      pack(av[2], av[3], num(av[4]));
      return 0;
    }
    if (ac == 5 && std::string(av[1]) == "restore") {
      restore(av[2], av[3], num(av[4]));
      return 0;
    }
    std::cerr << "usage: cix-backup pack INPUT NEW_REPO CHUNK_BYTES\n       "
                 "cix-backup restore REPO NEW_OUTPUT OUTPUT_CAP\n";
    return 2;
  } catch (const std::exception &e) {
    std::cerr << "cix-backup: " << e.what() << '\n';
    return 1;
  }
}
