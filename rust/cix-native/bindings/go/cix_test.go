package cix

import (
	"bytes"
	"errors"
	"testing"
)

func qualifiedOptions(t *testing.T) Options {
	t.Helper()
	options, err := DefaultOptions()
	if err != nil {
		t.Fatal(err)
	}
	options.OutputLimit = 1 << 20
	options.MemoryLimit = 8 << 20
	return options
}

func sample() []byte {
	data := make([]byte, 4099)
	for index := range data {
		data[index] = byte(uint(index*31) ^ uint(index>>3))
	}
	return data
}

func TestInstalledBufferRoundTrip(t *testing.T) {
	context, err := NewContext(qualifiedOptions(t))
	if err != nil {
		t.Fatal(err)
	}
	defer context.Close()
	source := sample()
	archive, err := context.Encode(source)
	if err != nil {
		t.Fatal(err)
	}
	restored, err := context.Decode(archive)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(restored, source) {
		t.Fatal("installed buffer ABI did not restore source bytes")
	}
}

func TestInstalledIndependentStreamRoundTrip(t *testing.T) {
	options := qualifiedOptions(t)
	encoder, err := NewEncoder(options)
	if err != nil {
		t.Fatal(err)
	}
	defer encoder.Close()
	source := sample()
	var archive []byte
	for offset, calls := 0, 0; offset < len(source); calls++ {
		if calls > 100000 {
			t.Fatal("encoder did not consume source")
		}
		end := offset + 13
		if end > len(source) {
			end = len(source)
		}
		part, progress, err := encoder.Process(source[offset:end], 1024)
		if err != nil {
			t.Fatal(err)
		}
		if progress.Consumed == 0 && progress.Produced == 0 {
			t.Fatal("encoder made no progress")
		}
		archive = append(archive, part...)
		offset += progress.Consumed
	}
	flushed, flushProgress, err := encoder.Flush(1024)
	if err != nil {
		t.Fatal(err)
	}
	if flushProgress.Produced > 1024 {
		t.Fatal("encoder flush returned an invalid produced count")
	}
	archive = append(archive, flushed...)
	for calls := 0; ; calls++ {
		if calls > 100000 {
			t.Fatal("encoder did not finish")
		}
		part, progress, err := encoder.Finish(1024)
		if err != nil {
			t.Fatal(err)
		}
		archive = append(archive, part...)
		if progress.State == Finished {
			break
		}
		if progress.Produced == 0 {
			t.Fatal("encoder finish made no progress")
		}
	}
	decoder, err := NewDecoder(options)
	if err != nil {
		t.Fatal(err)
	}
	defer decoder.Close()
	var restored []byte
	for offset, calls := 0, 0; offset < len(archive); calls++ {
		if calls > 100000 {
			t.Fatal("decoder did not consume archive")
		}
		end := offset + 11
		if end > len(archive) {
			end = len(archive)
		}
		part, progress, err := decoder.Process(archive[offset:end], 1024)
		if err != nil {
			t.Fatal(err)
		}
		if progress.Consumed == 0 && progress.Produced == 0 {
			t.Fatal("decoder made no progress")
		}
		restored = append(restored, part...)
		offset += progress.Consumed
	}
	for calls := 0; ; calls++ {
		if calls > 100000 {
			t.Fatal("decoder did not finish")
		}
		part, progress, err := decoder.Finish(1024)
		if err != nil {
			t.Fatal(err)
		}
		restored = append(restored, part...)
		if progress.State == Finished {
			break
		}
		if progress.Produced == 0 {
			t.Fatal("decoder finish made no progress")
		}
	}
	if !bytes.Equal(restored, source) {
		t.Fatal("installed independent stream did not restore source bytes")
	}
}

func TestCancelIsLocalAndResetClearsIt(t *testing.T) {
	encoder, err := NewEncoder(qualifiedOptions(t))
	if err != nil {
		t.Fatal(err)
	}
	defer encoder.Close()
	encoder.Cancel()
	if _, _, err := encoder.Process([]byte{1}, 64); err != ErrCancelled {
		t.Fatalf("cancel error = %v", err)
	}
	if err := encoder.Reset(); err != nil {
		t.Fatal(err)
	}
}

func TestWrapperAllocationLimitRejectsOversizedStreamBufferBeforeMake(t *testing.T) {
	const requested = 1 << 30
	_, err := outputBuffer(requested, requested, 64<<10)
	if err == nil {
		t.Fatal("oversized wrapper output buffer was accepted")
	}
	var native *Error
	if !errors.As(err, &native) || native.Status != StatusResourceLimit {
		t.Fatalf("allocation rejection = %v", err)
	}
}
