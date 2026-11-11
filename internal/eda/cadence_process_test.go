package eda

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The test binary is a headless child fixture, never the user's CAD program.
func TestCadenceProcessFixture(t *testing.T) {
	mode := os.Getenv("PACKAGESTUDIO_CADENCE_PROCESS_FIXTURE")
	if mode == "" {
		return
	}
	report := os.Getenv("PACKAGESTUDIO_CADENCE_PROCESS_REPORT")
	switch mode {
	case "atomic", "report-then-exit", "report-then-zero":
		f, err := os.Create(report + ".tmp")
		if err != nil {
			os.Exit(10)
		}
		_, _ = f.WriteString("protocol\tpackage-studio.cadence/1\n")
		time.Sleep(250 * time.Millisecond)
		_, _ = f.WriteString("complete\t1\n")
		_ = f.Close()
		if os.Rename(report+".tmp", report) != nil {
			os.Exit(11)
		}
		if mode == "report-then-exit" {
			os.Exit(17)
		}
		if mode == "report-then-zero" {
			return
		}
		time.Sleep(10 * time.Second)
	case "descendant":
		exe, _ := os.Executable()
		child := exec.Command(exe, "-test.run=^TestCadenceProcessFixture$")
		child.Env = append(os.Environ(), "PACKAGESTUDIO_CADENCE_PROCESS_FIXTURE=pipe-holder")
		child.Stdout, child.Stderr = os.Stdout, os.Stderr
		cadenceHideConsole(child)
		if child.Start() != nil {
			os.Exit(12)
		}
		_ = child.Process.Release()
		time.Sleep(10 * time.Second)
	case "pipe-holder":
		_ = os.WriteFile(report+".descendant-ready", []byte("ready"), 0600)
		time.Sleep(4 * time.Second)
		_ = os.WriteFile(report+".descendant-done", []byte("done"), 0600)
	case "timeout":
		time.Sleep(10 * time.Second)
	case "exit":
		return
	}
}

func TestCadenceRunnerWaitsForAtomicReportAndBoundsOwnedProcess(t *testing.T) {
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"atomic", "timeout", "exit", "report-then-exit", "report-then-zero", "descendant"} {
		t.Run(mode, func(t *testing.T) {
			dir := t.TempDir()
			report := filepath.Join(dir, "native.tsv")
			t.Setenv("PACKAGESTUDIO_CADENCE_PROCESS_FIXTURE", mode)
			t.Setenv("PACKAGESTUDIO_CADENCE_PROCESS_REPORT", report)
			duration := 3 * time.Second
			if mode == "timeout" {
				duration = 250 * time.Millisecond
			}
			if mode == "descendant" {
				duration = 750 * time.Millisecond
				// Let the bounded private descendant exit on its own before Go
				// removes the running test executable. No image-name/PID kill.
				t.Cleanup(func() {
					deadline := time.Now().Add(5 * time.Second)
					for time.Now().Before(deadline) {
						if _, e := os.Stat(report + ".descendant-done"); e == nil {
							time.Sleep(100 * time.Millisecond)
							return
						}
						time.Sleep(20 * time.Millisecond)
					}
					t.Error("private pipe-holder fixture did not finish")
				})
			}
			ctx, cancel := context.WithTimeout(context.Background(), duration)
			defer cancel()
			started := time.Now()
			pid, err := (cadenceExecRunner{}).Run(ctx, cadenceProcessSpec{Executable: exe, Args: []string{"-test.run=^TestCadenceProcessFixture$"}, Directory: dir, ReportPath: report, PIDPath: filepath.Join(dir, "engine.pid"), Phase: "fixture"})
			if pid <= 0 {
				t.Fatal("new fixture process was not started")
			}
			switch mode {
			case "atomic", "report-then-zero":
				if err != nil {
					t.Fatal(err)
				}
				data, err := os.ReadFile(report)
				if err != nil || !strings.HasSuffix(string(data), "complete\t1\n") || time.Since(started) < 250*time.Millisecond {
					t.Fatalf("partial report stopped the child: %q %v", data, err)
				}
			case "timeout":
				if !errors.Is(err, context.DeadlineExceeded) {
					t.Fatalf("lost context timeout: %v", err)
				}
			case "exit":
				if err == nil {
					t.Fatal("successful process exit without readback was accepted")
				}
			case "report-then-exit":
				if err == nil {
					t.Fatal("report hid a natural nonzero engine exit")
				}
			case "descendant":
				if !errors.Is(err, context.DeadlineExceeded) || time.Since(started) > 3500*time.Millisecond {
					t.Fatalf("descendant inherited pipe defeated context bound: %v duration=%s", err, time.Since(started))
				}
				if _, e := os.Stat(report + ".descendant-ready"); e != nil {
					t.Fatal("pipe-holder descendant did not start; test did not exercise inherited pipes")
				}
			}
		})
	}
}
