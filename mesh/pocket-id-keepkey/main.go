// keepkey puts a Pocket ID v1 signing key (data/keys/jwt_private_key.json) into a v2 database,
// through v2's own key provider, so the 1 -> 2 upgrade keeps the key every app already trusts.
// Upstream v2 drops file keys and generates a new one, which signs everyone out of every app.
//
// It is built inside Pocket ID's source tree at the release tag (build.sh), because it calls the
// same internal code as `pocket-id key-rotate`: ReplaceKey writes the kv row exactly as v2 does.
// Run it with the server's environment (ENCRYPTION_KEY_FILE at least) and the server stopped.
//
//	keepkey -db data/pocket-id.db                    print the kid stored in the database
//	keepkey -db data/pocket-id.db -key <v1 jwk>      replace the stored key with the v1 one
package main

import (
	"context"
	"crypto"
	"flag"
	"fmt"
	"os"

	"github.com/libtnb/sqlite"
	"github.com/lestrrat-go/jwx/v4/jwk"
	"gorm.io/gorm"
	"gorm.io/gorm/logger"

	"github.com/pocket-id/pocket-id/backend/internal/common"
	jwkutils "github.com/pocket-id/pocket-id/backend/internal/utils/jwk"
)

func main() {
	dbPath := flag.String("db", "data/pocket-id.db", "the v2 database (already migrated: start v2 once first)")
	keyPath := flag.String("key", "", "the v1 private key file; without it, only print what is stored")
	flag.Parse()

	if err := run(*dbPath, *keyPath); err != nil {
		fmt.Fprintln(os.Stderr, "keepkey:", err)
		os.Exit(1)
	}
}

func run(dbPath, keyPath string) error {
	ctx := context.Background()

	if _, err := os.Stat(dbPath); err != nil {
		return fmt.Errorf("no database at %s: %w", dbPath, err)
	}
	db, err := gorm.Open(sqlite.Open(dbPath), &gorm.Config{Logger: logger.Default.LogMode(logger.Silent)})
	if err != nil {
		return err
	}

	// Read the instance id rather than instanceid.Load, which would create one: a missing id means
	// this is not the database we think it is
	var instanceID string
	if err := db.Raw(`SELECT value FROM kv WHERE key = 'instance_id'`).Scan(&instanceID).Error; err != nil || instanceID == "" {
		return fmt.Errorf("no instance_id in %s (err %v)", dbPath, err)
	}

	provider, err := jwkutils.GetKeyProvider(db, &common.EnvConfig, instanceID)
	if err != nil {
		return err
	}

	stored, err := provider.LoadKey(ctx)
	if err != nil {
		return fmt.Errorf("stored key: %w", err)
	}
	fmt.Println("stored:", describe(stored))
	if keyPath == "" {
		return nil
	}

	raw, err := os.ReadFile(keyPath)
	if err != nil {
		return err
	}
	old, err := jwk.ParseKey(raw)
	if err != nil {
		return fmt.Errorf("v1 key: %w", err)
	}
	if _, ok := old.KeyID(); !ok {
		return fmt.Errorf("v1 key has no kid")
	}
	jwkutils.EnsureAlgInKey(old, "", "")
	fmt.Println("v1:    ", describe(old))

	if err := provider.ReplaceKey(ctx, old); err != nil {
		return err
	}

	// Read it back through the same path the server takes at boot
	back, err := provider.LoadKey(ctx)
	if err != nil {
		return fmt.Errorf("reading back: %w", err)
	}
	fmt.Println("now:   ", describe(back))
	if describe(back) != describe(old) {
		return fmt.Errorf("the key read back is not the v1 key")
	}
	return nil
}

// describe names a key by its kid, alg and public thumbprint, never its private parts
func describe(key jwk.Key) string {
	if key == nil {
		return "none"
	}
	kid, _ := key.KeyID()
	alg, _ := key.Algorithm()
	pub, err := key.PublicKey()
	if err != nil {
		return fmt.Sprintf("kid=%s alg=%v (no public key: %v)", kid, alg, err)
	}
	tp, err := pub.Thumbprint(crypto.SHA256)
	if err != nil {
		return fmt.Sprintf("kid=%s alg=%v (no thumbprint: %v)", kid, alg, err)
	}
	return fmt.Sprintf("kid=%s alg=%v thumbprint=%x", kid, alg, tp)
}
