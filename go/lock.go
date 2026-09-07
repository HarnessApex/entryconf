package entryconf

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

const (
	lockFileName = ".entryconf.lock"

	// lockStaleAfter is how old a lock file must be before a waiter may treat
	// it as abandoned by a crashed writer (SPEC §10.7). A commit holds the lock
	// for milliseconds.
	lockStaleAfter = 30 * time.Second

	// DefaultLockTimeout is how long Commit waits for the directory lock when
	// CommitOptions.LockTimeout is zero.
	DefaultLockTimeout = 5 * time.Second
)

// acquireLock implements the SPEC §10.7 lock protocol: exclusive-create
// <dir>/.entryconf.lock, retrying until timeout, breaking a lock older than
// lockStaleAfter by renaming it away first so that at most one waiter breaks
// a given stale lock.
func acquireLock(dir string, timeout time.Duration) (release func(), err error) {
	path := filepath.Join(dir, lockFileName)
	deadline := time.Now().Add(timeout)
	wait := 5 * time.Millisecond
	for {
		f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if err == nil {
			fmt.Fprintf(f, "{\"pid\": %d, \"created\": %q}\n", os.Getpid(), time.Now().UTC().Format(time.RFC3339))
			f.Close()
			return func() { os.Remove(path) }, nil
		}
		if !errors.Is(err, os.ErrExist) {
			return nil, wrapf(CodeLocked, err, "cannot create lock file %q", path)
		}
		if info, statErr := os.Stat(path); statErr == nil && time.Since(info.ModTime()) > lockStaleAfter {
			// Presumed abandoned: rename it to a unique name and delete that.
			// Only one waiter's rename succeeds, so the lock is broken once.
			stale := path + "." + randomSuffix()
			if os.Rename(path, stale) == nil {
				os.Remove(stale)
			}
			continue
		}
		if time.Now().After(deadline) {
			return nil, errf(CodeLocked, "lock file %q is held by another writer (waited %s)", path, timeout)
		}
		time.Sleep(wait)
		if wait < 100*time.Millisecond {
			wait *= 2
		}
	}
}

func randomSuffix() string {
	var buf [6]byte
	if _, err := rand.Read(buf[:]); err != nil {
		return fmt.Sprintf("%d", time.Now().UnixNano())
	}
	return hex.EncodeToString(buf[:])
}

// atomicWrite replaces target's content with data (SPEC §10.7 step 3): a
// temporary file in the target's directory, flushed, given the target's
// permission bits, and renamed over it. A symlinked target is resolved first
// so the link survives and the file it points at is what changes. On any
// failure the temporary file is removed and the target is untouched.
func atomicWrite(target string, data []byte) error {
	resolved, err := filepath.EvalSymlinks(target)
	if err != nil {
		return wrapf(CodeWrite, err, "cannot resolve %q", target)
	}
	info, err := os.Stat(resolved)
	if err != nil {
		return wrapf(CodeWrite, err, "cannot stat %q", resolved)
	}
	dir, base := filepath.Split(resolved)
	tmp, err := os.CreateTemp(dir, "."+base+".entryconf-tmp-*")
	if err != nil {
		return wrapf(CodeWrite, err, "cannot create a temporary file beside %q", resolved)
	}
	tmpPath := tmp.Name()
	fail := func(step string, cause error) error {
		tmp.Close()
		os.Remove(tmpPath)
		return wrapf(CodeWrite, cause, "%s %q", step, resolved)
	}
	if _, err := tmp.Write(data); err != nil {
		return fail("cannot write replacement for", err)
	}
	if err := tmp.Sync(); err != nil {
		return fail("cannot flush replacement for", err)
	}
	if err := tmp.Chmod(info.Mode().Perm()); err != nil {
		// Windows has no permission bits to preserve; elsewhere this matters.
		if !isWindows() {
			return fail("cannot set permissions on replacement for", err)
		}
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpPath)
		return wrapf(CodeWrite, err, "cannot close replacement for %q", resolved)
	}
	if err := os.Rename(tmpPath, resolved); err != nil {
		os.Remove(tmpPath)
		return wrapf(CodeWrite, err, "cannot replace %q", resolved)
	}
	// Best effort: make the rename itself durable where the platform allows.
	if d, err := os.Open(dir); err == nil {
		_ = d.Sync()
		d.Close()
	}
	return nil
}
